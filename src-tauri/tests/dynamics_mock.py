#!/usr/bin/env python3
"""A minimal in-memory mock of the Dataverse Web API v9.2 (web resources +
PublishXml) and the Microsoft Entra client-credentials token endpoint, used by
the `live_dynamics_roundtrip` integration test in `src/remotefs/dynamics.rs`.

It is NOT a faithful Dataverse — just enough of the shapes Faro's
DynamicsSession / DynamicsFs actually call, so the whole client path (Bearer
auth via the client-credentials exchange, the WhoAmI connect probe + label,
the paged webresourceset query with $select/$filter/@odata.nextLink, row
GET/POST/PATCH/DELETE with base64 content, PublishXml recording, managed-row
write refusal, and the 429 Retry-After retry) can be exercised without a real
environment.

Run:  python src-tauri/tests/dynamics_mock.py <port>
Endpoints:
  POST   /{tenant}/oauth2/v2.0/token              client-credentials exchange
  GET    /api/data/v9.2/WhoAmI                    connect probe + label
  GET    /api/data/v9.2/webresourceset?$select=…  paged listing (page size 2)
  GET    /api/data/v9.2/webresourceset?$filter=name eq '…'
  GET    /api/data/v9.2/webresourceset({id})?$select=content
  POST   /api/data/v9.2/webresourceset            create (204 + OData-EntityId)
  PATCH  /api/data/v9.2/webresourceset({id})      update content
  DELETE /api/data/v9.2/webresourceset({id})
  POST   /api/data/v9.2/PublishXml                recorded for the test
  GET    /__publishes                             test hook: recorded publishes
Auth: API calls need `Authorization: Bearer DYN-TOKEN` (from the token
endpoint), else 401. The token endpoint and /__publishes are unauthenticated.
429 hook: the first POST whose body name contains "rl429" fails once with
`429 + Retry-After: 0.1`, then succeeds on the client's retry.
Managed hook: writes/deletes on the ismanaged row fail 400 (the client also
refuses them client-side — the mock is the backstop).
"""
import base64
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, unquote, urlparse

VALID_TOKEN = "DYN-TOKEN"
TENANT_RE = re.compile(r"^/[^/]+/oauth2/v2\.0/token$")
API = "/api/data/v9.2"

ISO = "2024-07-15T09:30:00Z"
USER_ID = "11111111-2222-3333-4444-555555555555"
ORG_ID = "99999999-8888-7777-6666-555555555555"

# Set in __main__ so OData-EntityId / nextLink can point back here.
BASE_URL = "http://127.0.0.1:8844"

NEXT_NUM = [100]


def b64(data):
    return base64.b64encode(data).decode()


def row(name, res_type, data, managed=False):
    rid = f"wr{NEXT_NUM[0]:04d}-0000-0000-0000-000000000000"
    NEXT_NUM[0] += 1
    return {
        "webresourceid": rid,
        "name": name,
        "webresourcetype": res_type,
        "modifiedon": ISO,
        "ismanaged": managed,
        "_data": data,  # bytes, never serialized as such
    }


# id -> row. Four seed rows page at 2, forcing the client's nextLink loop.
RESOURCES = {}
for r in [
    row("new_/js/form.js", 3, b"console.log('form');"),
    row("new_/css/main.css", 2, b"body{}"),
    row("new_/img/logo.png", 5, b"png-bytes"),
    row("amp_/lib/managed.js", 3, b"// managed", managed=True),
]:
    RESOURCES[r["webresourceid"]] = r

PAGE_LIMIT = 2  # tiny server-side pages force the client's nextLink loop
RL429_SEEN = set()  # names that already burned their one 429
PUBLISHES = []  # every PublishXml batch, in order (test hook)


def public_row(r, select):
    """The wire shape of one row, honoring $select (content only on request)."""
    fields = select.split(",") if select else [
        "name", "webresourceid", "webresourcetype", "modifiedon", "ismanaged"]
    out = {}
    for f in fields:
        f = f.strip()
        if f == "content":
            out["content"] = b64(r["_data"])
        elif f in r:
            out[f] = r[f]
    return out


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj=None, headers=None):
        body = b"" if obj is None else json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def _authed(self):
        if self.headers.get("Authorization", "") != f"Bearer {VALID_TOKEN}":
            self._send(401, {"error": {"code": "0x80040220",
                                       "message": "Unauthorized"}})
            return False
        return True

    def _json_body(self):
        n = int(self.headers.get("Content-Length", 0))
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except json.JSONDecodeError:
            return {}

    def _odata_error(self, code, message, http=400):
        self._send(http, {"error": {"code": code, "message": message}})

    def _row_route(self):
        """`/api/data/v9.2/webresourceset({id})` -> id, else None."""
        u = urlparse(self.path)
        m = re.match(rf"^{re.escape(API)}/webresourceset\(([^)]+)\)$", u.path)
        return m.group(1) if m else None

    def _set_route(self):
        """`/api/data/v9.2/webresourceset` (no id) -> True, else None."""
        u = urlparse(self.path)
        if u.path == f"{API}/webresourceset":
            return True
        return None

    def _query(self):
        return parse_qs(urlparse(self.path).query)

    # ---- handlers ----

    def _token(self):
        # Client-credentials exchange: validate the form the client sends
        # (this is what proves the tenant:client_id:client_secret blob was
        # parsed correctly end to end).
        n = int(self.headers.get("Content-Length", 0))
        form = parse_qs(self.rfile.read(n).decode())
        if form.get("grant_type", [""])[0] != "client_credentials":
            return self._send(400, {"error": "unsupported_grant_type"})
        if form.get("client_id", [""])[0] != "test-client-id" or \
           form.get("client_secret", [""])[0] != "test-secret":
            return self._send(401, {"error": "invalid_client",
                                    "error_description": "bad client credentials"})
        self._send(200, {"access_token": VALID_TOKEN, "token_type": "Bearer",
                         "expires_in": 3600})

    def _whoami(self):
        self._send(200, {
            "@odata.context": f"{BASE_URL}{API}/$metadata#WhoAmI",
            "BusinessUnitId": "33333333-4444-5555-6666-777777777777",
            "UserId": USER_ID,
            "OrganizationId": ORG_ID,
        })

    def _list(self):
        q = self._query()
        select = q.get("$select", [""])[0]
        rows = list(RESOURCES.values())
        # $filter=name eq '…' — the write path's create-vs-update lookup.
        filt = q.get("$filter", [None])[0]
        if filt:
            m = re.match(r"name eq '((?:[^']|'')*)'", filt)
            if m:
                name = m.group(1).replace("''", "'")
                rows = [r for r in rows if r["name"] == name]
            else:
                return self._odata_error("0x80060888", f"bad $filter: {filt}")
        # Paging: $skip drives the pages; the nextLink is absolute (the real
        # API's skiptoken is opaque — $skip is the mock's equivalent).
        rows = sorted(rows, key=lambda r: r["webresourceid"])
        try:
            skip = int(q.get("$skip", ["0"])[0])
        except ValueError:
            skip = 0
        page = rows[skip:skip + PAGE_LIMIT]
        body = {"value": [public_row(r, select) for r in page]}
        if skip + PAGE_LIMIT < len(rows):
            nq = f"$skip={skip + PAGE_LIMIT}"
            if select:
                nq = f"$select={select}&{nq}"
            body["@odata.nextLink"] = f"{BASE_URL}{API}/webresourceset?{nq}"
        self._send(200, body)

    def _get_content(self, rid):
        r = RESOURCES.get(rid)
        if r is None:
            return self._odata_error("0x80040217", f"webresource {rid} not found",
                                     http=404)
        select = self._query().get("$select", [""])[0]
        self._send(200, public_row(r, select))

    def _create(self):
        body = self._json_body()
        name = body.get("name", "")
        # 429-injection hook: names containing "rl429" fail once.
        if "rl429" in name and name not in RL429_SEEN:
            RL429_SEEN.add(name)
            return self._send(429, {"error": {"code": "0x80072322",
                                              "message": "rate limited"}},
                              headers={"Retry-After": "0.1"})
        if any(r["name"] == name for r in RESOURCES.values()):
            return self._odata_error("0x80044330",
                                     f"a web resource named {name} already exists")
        try:
            data = base64.b64decode(body.get("content", ""))
        except Exception:
            return self._odata_error("0x80040203", "content must be base64")
        r = row(name, int(body.get("webresourcetype", 0)), data)
        RESOURCES[r["webresourceid"]] = r
        entity_url = f"{BASE_URL}{API}/webresourceset({r['webresourceid']})"
        self._send(204, headers={"OData-EntityId": entity_url})

    def _update(self, rid):
        r = RESOURCES.get(rid)
        if r is None:
            return self._odata_error("0x80040217", f"webresource {rid} not found",
                                     http=404)
        if r["ismanaged"]:
            return self._odata_error(
                "0x8004f503",
                f"managed web resource {r['name']} cannot be updated")
        body = self._json_body()
        try:
            r["_data"] = base64.b64decode(body.get("content", ""))
        except Exception:
            return self._odata_error("0x80040203", "content must be base64")
        self._send(204)

    def _delete(self, rid):
        r = RESOURCES.pop(rid, None)
        if r is None:
            return self._odata_error("0x80040217", f"webresource {rid} not found",
                                     http=404)
        if r["ismanaged"]:
            RESOURCES[rid] = r  # put it back
            return self._odata_error(
                "0x8004f503",
                f"managed web resource {r['name']} cannot be deleted")
        self._send(204)

    def _publish(self):
        body = self._json_body()
        PUBLISHES.append(body.get("ParameterXml", ""))
        self._send(204)

    # ---- routing ----

    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/__publishes":
            return self._send(200, {"publishes": PUBLISHES})
        if not u.path.startswith(API):
            return self._odata_error("0x80060888", "Not Found", http=404)
        if not self._authed():
            return
        if u.path == f"{API}/WhoAmI":
            return self._whoami()
        rid = self._row_route()
        if rid is not None:
            return self._get_content(unquote(rid))
        if self._set_route():
            return self._list()
        self._odata_error("0x80060888", "Not Found", http=404)

    def do_POST(self):
        u = urlparse(self.path)
        if TENANT_RE.match(u.path):
            return self._token()
        if not u.path.startswith(API):
            return self._odata_error("0x80060888", "Not Found", http=404)
        if not self._authed():
            return
        if u.path == f"{API}/PublishXml":
            return self._publish()
        if self._set_route():
            return self._create()
        self._odata_error("0x80060888", "Not Found", http=404)

    def do_PATCH(self):
        rid = self._row_route()
        if rid is None:
            return self._odata_error("0x80060888", "Not Found", http=404)
        if not self._authed():
            return
        self._update(unquote(rid))

    def do_DELETE(self):
        rid = self._row_route()
        if rid is None:
            return self._odata_error("0x80060888", "Not Found", http=404)
        if not self._authed():
            return
        self._delete(unquote(rid))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8844
    BASE_URL = f"http://127.0.0.1:{port}"
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
