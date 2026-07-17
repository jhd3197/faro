#!/usr/bin/env python3
"""Minimal in-memory mock of the Box API v2 + OAuth token endpoint, for the
`live_box_roundtrip` test in `src/remotefs/boxdrive.rs`.

Box is ID-addressed (root id "0"), so this models an id→node tree and just enough
endpoints to exercise Faro's resolver + RemoteFs/transfer ops (token exchange,
401→refresh, list folder items, create folder, multipart upload, download, move
via PUT, recursive delete) without a real account.

Run:  python src-tauri/tests/box_mock.py <port>
Auth: Box calls need Authorization: Bearer ACCESS1|ACCESS-REFRESHED.
"""
import hashlib
import json
import sys
from urllib.parse import urlparse, parse_qs
from http.server import BaseHTTPRequestHandler, HTTPServer

VALID_TOKENS = {"ACCESS1", "ACCESS-REFRESHED"}
# id -> {name, type:"folder"|"file", parent:id, content: bytes|None}
NODES = {}
_next = [0]


def new_id():
    _next[0] += 1
    return str(1000 + _next[0])


def meta(fid):
    n = NODES[fid]
    m = {"id": fid, "name": n["name"], "type": n["type"]}
    if n["type"] == "file" and n["content"] is not None:
        m["size"] = len(n["content"])
        m["modified_at"] = "2024-07-15T09:30:00-07:00"
        m["sha1"] = hashlib.sha1(n["content"]).hexdigest()
    return m


def parse_form(body, boundary):
    """Return {field_name: bytes} for a multipart/form-data body."""
    out = {}
    for seg in body.split(b"--" + boundary.encode()):
        i = seg.find(b"\r\n\r\n")
        if i < 0:
            continue
        head = seg[:i].decode(errors="replace")
        if 'name="' not in head:
            continue
        name = head.split('name="', 1)[1].split('"', 1)[0]
        out[name] = seg[i + 4:].rstrip(b"\r\n")
    return out


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj=None, raw=None):
        body = raw if raw is not None else json.dumps(obj or {}).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/octet-stream" if raw is not None
                         else "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def _authed(self):
        if self.headers.get("Authorization", "")[7:] not in VALID_TOKENS:
            self._send(401, {"code": "unauthorized"})
            return False
        return True

    def _body(self):
        n = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(n) if n else b""

    def do_POST(self):
        u = urlparse(self.path)
        raw = self._body()
        if u.path.endswith("/token"):
            form = dict(kv.split("=", 1) for kv in raw.decode().split("&") if "=" in kv)
            if form.get("grant_type") == "refresh_token":
                return self._send(200, {"access_token": "ACCESS-REFRESHED",
                                        "token_type": "bearer", "expires_in": 3600})
            return self._send(200, {"access_token": "ACCESS1", "refresh_token": "REFRESH1",
                                    "token_type": "bearer", "expires_in": 3600})
        if not self._authed():
            return
        # Uploads: /files/content (new) or /files/{id}/content (new version).
        if "/files/content" in u.path or u.path.endswith("/content"):
            boundary = self.headers.get("Content-Type", "").split("boundary=", 1)[1]
            fields = parse_form(raw, boundary)
            media = fields.get("file", b"")
            if u.path.endswith("/files/content"):
                attrs = json.loads(fields.get("attributes", b"{}"))
                fid = new_id()
                NODES[fid] = {"name": attrs["name"], "type": "file",
                              "parent": attrs["parent"]["id"], "content": media}
            else:
                fid = u.path.rsplit("/", 2)[-2]           # /files/{id}/content
                NODES[fid]["content"] = media
            return self._send(201, {"total_count": 1, "entries": [meta(fid)]})
        if u.path.endswith("/folders"):
            m = json.loads(raw or b"null")
            fid = new_id()
            NODES[fid] = {"name": m["name"], "type": "folder",
                          "parent": m["parent"]["id"], "content": None}
            return self._send(201, meta(fid))
        self._send(400, {"code": "unknown"})

    def do_PUT(self):
        if not self._authed():
            return
        raw = self._body()
        fid = urlparse(self.path).path.rsplit("/", 1)[-1]
        m = json.loads(raw or b"null")
        if "name" in m:
            NODES[fid]["name"] = m["name"]
        if "parent" in m:
            NODES[fid]["parent"] = m["parent"]["id"]
        self._send(200, meta(fid))

    def do_DELETE(self):
        if not self._authed():
            return
        fid = urlparse(self.path).path.rsplit("/", 1)[-1]

        def rm(x):
            for c in [k for k, n in NODES.items() if n["parent"] == x]:
                rm(c)
            NODES.pop(x, None)
        rm(fid)
        self._send(204)

    def do_GET(self):
        u = urlparse(self.path)
        if not self._authed():
            return
        if u.path.endswith("/users/me"):
            return self._send(200, {"login": "tester@example.com", "name": "Test User"})
        if u.path.endswith("/content"):                   # /files/{id}/content
            fid = u.path.rsplit("/", 2)[-2]
            return self._send(200, raw=NODES.get(fid, {}).get("content") or b"")
        if "/folders/" in u.path and u.path.endswith("/items"):
            parent = u.path.split("/folders/", 1)[1].split("/", 1)[0]
            entries = [meta(k) for k, n in NODES.items() if n["parent"] == parent]
            return self._send(200, {"entries": entries, "total_count": len(entries),
                                    "offset": 0, "limit": 1000})
        # /files/{id}?fields=size
        fid = u.path.rsplit("/", 1)[-1]
        if fid in NODES:
            return self._send(200, meta(fid))
        self._send(404, {"code": "not_found"})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8823
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
