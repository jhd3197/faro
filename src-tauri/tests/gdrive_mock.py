#!/usr/bin/env python3
"""Minimal in-memory mock of the Google Drive v3 API + OAuth token endpoint, for
the `live_gdrive_roundtrip` test in `src/remotefs/gdrive.rs`.

Drive is ID-addressed, so this models an id→node tree and just enough endpoints to
exercise Faro's path↔id resolver and the RemoteFs/transfer ops (token exchange,
401→refresh, list via q, create folder, multipart create, media update, download,
move via addParents/removeParents, delete) without a real account.

Run:  python src-tauri/tests/gdrive_mock.py <port>
Auth: Drive calls need Authorization: Bearer ACCESS1|ACCESS-REFRESHED.
"""
import hashlib
import json
import sys
from urllib.parse import urlparse, parse_qs, unquote
from http.server import BaseHTTPRequestHandler, HTTPServer

VALID_TOKENS = {"ACCESS1", "ACCESS-REFRESHED"}
FOLDER_MIME = "application/vnd.google-apps.folder"
# id -> {name, mimeType, parents:[ids], content: bytes|None}
NODES = {}
_next = [0]


def new_id():
    _next[0] += 1
    return f"id{_next[0]}"


def meta(fid):
    n = NODES[fid]
    m = {"id": fid, "name": n["name"], "mimeType": n["mimeType"],
         "modifiedTime": "2024-07-15T09:30:00Z"}
    if n["mimeType"] != FOLDER_MIME and n["content"] is not None:
        m["size"] = str(len(n["content"]))
        m["md5Checksum"] = hashlib.md5(n["content"]).hexdigest()
    return m


def parse_multipart(body, boundary):
    delim = b"--" + boundary.encode()
    segs = body.split(delim)

    def payload(seg):
        i = seg.find(b"\r\n\r\n")
        return seg[i + 4:].rstrip(b"\r\n")

    return json.loads(payload(segs[1])), payload(segs[2])


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
            self._send(401, {"error": {"code": 401, "message": "invalid"}})
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
                                        "token_type": "Bearer", "expires_in": 3600})
            return self._send(200, {"access_token": "ACCESS1", "refresh_token": "REFRESH1",
                                    "token_type": "Bearer", "expires_in": 3600})
        if not self._authed():
            return
        q = parse_qs(u.query)
        if u.path.endswith("/files") and q.get("uploadType", [""])[0] == "multipart":
            ctype = self.headers.get("Content-Type", "")
            boundary = ctype.split("boundary=", 1)[1]
            m, media = parse_multipart(raw, boundary)
            fid = new_id()
            NODES[fid] = {"name": m["name"], "mimeType": "application/octet-stream",
                          "parents": m.get("parents", []), "content": media}
            return self._send(200, meta(fid))
        if u.path.endswith("/files"):                       # create folder (metadata only)
            m = json.loads(raw or b"null")
            fid = new_id()
            NODES[fid] = {"name": m["name"], "mimeType": m.get("mimeType", FOLDER_MIME),
                          "parents": m.get("parents", []), "content": None}
            return self._send(200, meta(fid))
        self._send(400, {"error": {"message": "unknown"}})

    def do_PATCH(self):
        if not self._authed():
            return
        u = urlparse(self.path)
        raw = self._body()
        fid = u.path.rsplit("/", 1)[-1]
        q = parse_qs(u.query)
        if q.get("uploadType", [""])[0] == "media":         # update content
            NODES[fid]["content"] = raw
            return self._send(200, meta(fid))
        body = json.loads(raw or b"null")                   # rename / move
        if "name" in body:
            NODES[fid]["name"] = body["name"]
        add = q.get("addParents", [None])[0]
        rem = q.get("removeParents", [None])[0]
        if rem and rem in NODES[fid]["parents"]:
            NODES[fid]["parents"].remove(rem)
        if add and add not in NODES[fid]["parents"]:
            NODES[fid]["parents"].append(add)
        self._send(200, meta(fid))

    def do_DELETE(self):
        if not self._authed():
            return
        fid = urlparse(self.path).path.rsplit("/", 1)[-1]

        def rm(x):
            for c in [k for k, n in NODES.items() if x in n["parents"]]:
                rm(c)
            NODES.pop(x, None)
        rm(fid)
        self._send(204)

    def do_GET(self):
        u = urlparse(self.path)
        if not self._authed():
            return
        if u.path.endswith("/about"):
            return self._send(200, {"user": {"emailAddress": "tester@example.com",
                                             "displayName": "Test User"}})
        q = parse_qs(u.query)
        if u.path.endswith("/files"):                       # list by q
            query = q.get("q", [""])[0]
            parent = query.split("'", 2)[1] if "in parents" in query else "root"
            name = None
            if "name = '" in query:
                name = query.split("name = '", 1)[1].split("'", 1)[0]
            files = []
            for fid, n in NODES.items():
                if parent in n["parents"] and (name is None or n["name"] == name):
                    files.append(meta(fid))
            return self._send(200, {"files": files})
        # /files/{id}
        fid = u.path.rsplit("/", 1)[-1]
        if fid not in NODES:
            return self._send(404, {"error": {"code": 404}})
        if q.get("alt", [""])[0] == "media":
            return self._send(200, raw=NODES[fid]["content"] or b"")
        self._send(200, meta(fid))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8822
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
