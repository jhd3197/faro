#!/usr/bin/env python3
"""Minimal in-memory mock of the Microsoft Graph OneDrive API + OAuth token
endpoint, for the `live_onedrive_roundtrip` test in `src/remotefs/onedrive.rs`.

Just enough of the shapes Faro's OneDriveSession/OneDriveFs call to exercise the
whole client path (token exchange, 401→refresh, list/create/upload/download/
move/delete) without a real Microsoft account.

Run:  python src-tauri/tests/onedrive_mock.py <port>
Auth: Graph calls need Authorization: Bearer ACCESS1|ACCESS-REFRESHED.
"""
import json
import sys
from urllib.parse import unquote
from http.server import BaseHTTPRequestHandler, HTTPServer

VALID_TOKENS = {"ACCESS1", "ACCESS-REFRESHED"}
FILES = {}     # "/a/b" -> bytes
FOLDERS = set()
ROOT = "/me/drive/root"


def meta(path, is_dir):
    name = path.rsplit("/", 1)[-1] or path
    m = {"name": name, "id": "id:" + path,
         "lastModifiedDateTime": "2024-07-15T09:30:00Z", "cTag": "ctag:" + path}
    if is_dir:
        m["folder"] = {"childCount": 0}
        m["size"] = 0
    else:
        m["file"] = {"mimeType": "application/octet-stream"}
        m["size"] = len(FILES.get(path, b""))
    return m


def children(parent):
    prefix = (parent + "/") if parent else "/"
    out = []
    for p in list(FOLDERS):
        if p.startswith(prefix) and "/" not in p[len(prefix):] and p[len(prefix):]:
            out.append(meta(p, True))
    for p in list(FILES):
        if p.startswith(prefix) and "/" not in p[len(prefix):] and p[len(prefix):]:
            out.append(meta(p, False))
    return out


def parse_ref(path):
    """(item_path, suffix) from a Graph URL path. item_path is decoded, '' at root."""
    if path == ROOT:
        return "", ""
    if path == ROOT + "/children":
        return "", "children"
    if path.startswith(ROOT + ":/"):
        rest = path[len(ROOT) + 2:]           # "a/b:/children" | "a/b:"
        item_enc, _, after = rest.partition(":")
        suffix = after.lstrip("/")
        return "/" + unquote(item_enc), suffix
    return None, None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj=None, raw=None, headers=None):
        body = raw if raw is not None else json.dumps(obj or {}).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/octet-stream" if raw is not None
                         else "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def _authed(self):
        token = self.headers.get("Authorization", "")[7:]
        if token not in VALID_TOKENS:
            self._send(401, {"error": {"code": "InvalidAuthenticationToken"}})
            return False
        return True

    def _body(self):
        n = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(n) if n else b""

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        raw = self._body()

        if path.endswith("/oauth2/token") or path.endswith("/token"):
            form = dict(kv.split("=", 1) for kv in raw.decode().split("&") if "=" in kv)
            if form.get("grant_type") == "refresh_token":
                return self._send(200, {"access_token": "ACCESS-REFRESHED",
                                        "token_type": "Bearer", "expires_in": 3600})
            return self._send(200, {"access_token": "ACCESS1", "refresh_token": "REFRESH1",
                                    "token_type": "Bearer", "expires_in": 3600})

        # Resumable upload chunk PUTs land here (see do_PUT); POST handles RPC.
        if not self._authed():
            return
        item, suffix = parse_ref(path)
        if suffix == "children":                      # create folder
            body = json.loads(raw or b"null")
            new = (item.rstrip("/") + "/" + body["name"]) if item else "/" + body["name"]
            FOLDERS.add(new)
            return self._send(201, meta(new, True))
        if suffix == "createUploadSession":
            return self._send(200, {"uploadUrl": f"http://{self.headers['Host']}"
                                    f"/upload-session?item={item}"})
        self._send(400, {"error": {"code": "unknown"}})

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if not self._authed():
            return
        if path == "/me":
            return self._send(200, {"userPrincipalName": "tester@example.com",
                                    "displayName": "Test User", "id": "u1"})
        item, suffix = parse_ref(path)
        if suffix == "children":
            return self._send(200, {"value": children(item)})
        if suffix == "content":
            data = FILES.get(item)
            if data is None:
                return self._send(404, {"error": {"code": "itemNotFound"}})
            return self._send(200, raw=data)
        if suffix == "":                              # metadata
            if item in FILES:
                return self._send(200, meta(item, False))
            if item in FOLDERS:
                return self._send(200, meta(item, True))
            return self._send(404, {"error": {"code": "itemNotFound"}})
        self._send(404, {"error": {"code": "itemNotFound"}})

    def do_PUT(self):
        path = self.path.split("?", 1)[0]
        raw = self._body()
        # Resumable upload chunk — the uploadUrl carries ?item=… and needs no auth.
        if path == "/upload-session":
            from urllib.parse import urlparse, parse_qs
            item = parse_qs(urlparse(self.path).query).get("item", [""])[0]
            rng = self.headers.get("Content-Range", "")
            # "bytes start-end/total"
            span, _, total = rng[len("bytes "):].partition("/")
            start, _, end = span.partition("-")
            buf = FILES.get(item, b"") if int(start) else b""
            FILES[item] = buf + raw
            if int(end) + 1 >= int(total):
                return self._send(201, meta(item, False))
            return self._send(202, {"nextExpectedRanges": [f"{int(end)+1}-"]})
        # Simple upload PUT …/content.
        if not self._authed():
            return
        item, suffix = parse_ref(path)
        if suffix == "content":
            FILES[item] = raw
            return self._send(201, meta(item, False))
        self._send(400, {"error": {"code": "unknown"}})

    def do_PATCH(self):
        if not self._authed():
            return
        raw = self._body()
        item, _ = parse_ref(self.path.split("?", 1)[0])
        body = json.loads(raw or b"null")
        name = body["name"]
        pref = body.get("parentReference", {}).get("path", "/drive/root:")
        parent = "" if pref == "/drive/root:" else "/" + unquote(pref[len("/drive/root:/"):])
        new = (parent.rstrip("/") + "/" + name) if parent else "/" + name
        if item in FILES:
            FILES[new] = FILES.pop(item)
        elif item in FOLDERS:
            FOLDERS.discard(item)
            FOLDERS.add(new)
        self._send(200, meta(new, new in FOLDERS))

    def do_DELETE(self):
        if not self._authed():
            return
        item, _ = parse_ref(self.path.split("?", 1)[0])
        for f in list(FILES):
            if f == item or f.startswith(item + "/"):
                FILES.pop(f, None)
        for d in list(FOLDERS):
            if d == item or d.startswith(item + "/"):
                FOLDERS.discard(d)
        self._send(204)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8821
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
