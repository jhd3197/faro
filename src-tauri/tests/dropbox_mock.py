#!/usr/bin/env python3
"""A minimal in-memory mock of the Dropbox OAuth token endpoint + API v2, used by
the `live_dropbox_roundtrip` integration test in `src/remotefs/dropbox.rs`.

It is NOT a faithful Dropbox — just enough of the shapes Faro's DropboxSession /
DropboxFs actually call, so the whole client path (token exchange, 401→refresh
retry, list/upload/download/move/delete) can be exercised without a real account.

Run:  python src-tauri/tests/dropbox_mock.py <port>
Endpoints (same host serves api + content bases):
  POST /oauth2/token                     grant_type=authorization_code | refresh_token
  POST /2/users/get_current_account
  POST /2/files/list_folder[/continue]
  POST /2/files/get_metadata
  POST /2/files/create_folder_v2 | move_v2 | delete_v2
  POST /2/files/upload    (content; Dropbox-API-Arg header, octet-stream body)
  POST /2/files/download  (content; Dropbox-API-Arg header)
Auth: API calls need `Authorization: Bearer ACCESS1|ACCESS-REFRESHED`, else 401.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

VALID_TOKENS = {"ACCESS1", "ACCESS-REFRESHED"}

# In-memory tree. files: path -> bytes; folders: set of paths. Paths look like
# "/a/b"; root is "".
FILES = {}
FOLDERS = set()


def meta(path, is_dir):
    name = path.rsplit("/", 1)[-1] or path
    if is_dir:
        return {".tag": "folder", "name": name,
                "path_display": path, "path_lower": path.lower(), "id": "id:" + path}
    data = FILES.get(path, b"")
    return {".tag": "file", "name": name, "path_display": path,
            "path_lower": path.lower(), "size": len(data),
            "server_modified": "2024-07-15T09:30:00Z", "rev": "01rev" + str(len(data)),
            "id": "id:" + path}


def direct_children(parent):
    prefix = (parent + "/") if parent else "/"
    out = []
    for p in list(FOLDERS):
        if p.startswith(prefix) and "/" not in p[len(prefix):] and p[len(prefix):]:
            out.append(meta(p, True))
    for p in list(FILES):
        if p.startswith(prefix) and "/" not in p[len(prefix):] and p[len(prefix):]:
            out.append(meta(p, False))
    return out


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
        self.wfile.write(body)

    def _authed(self):
        auth = self.headers.get("Authorization", "")
        token = auth[7:] if auth.startswith("Bearer ") else ""
        if token not in VALID_TOKENS:
            self._send(401, {"error_summary": "invalid_access_token/"})
            return False
        return True

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(n) if n else b""
        path = self.path.split("?", 1)[0]

        if path == "/oauth2/token":
            form = dict(kv.split("=", 1) for kv in raw.decode().split("&") if "=" in kv)
            if form.get("grant_type") == "refresh_token":
                return self._send(200, {"access_token": "ACCESS-REFRESHED",
                                        "token_type": "bearer", "expires_in": 3600})
            return self._send(200, {"access_token": "ACCESS1", "refresh_token": "REFRESH1",
                                    "token_type": "bearer", "expires_in": 3600,
                                    "account_id": "dbid:1"})

        # Content endpoints carry their arg in a header, body is the file bytes.
        if path == "/2/files/upload":
            if not self._authed():
                return
            arg = json.loads(self.headers.get("Dropbox-API-Arg", "{}"))
            FILES[arg["path"]] = raw
            return self._send(200, meta(arg["path"], False))
        if path == "/2/files/download":
            if not self._authed():
                return
            arg = json.loads(self.headers.get("Dropbox-API-Arg", "{}"))
            data = FILES.get(arg["path"])
            if data is None:
                return self._send(409, {"error_summary": "path/not_found/"})
            return self._send(200, raw=data,
                              headers={"Dropbox-API-Result": json.dumps(meta(arg["path"], False))})

        # RPC endpoints: JSON body.
        if not self._authed():
            return
        body = json.loads(raw.decode() or "null")

        if path == "/2/users/get_current_account":
            return self._send(200, {"account_id": "dbid:1", "email": "tester@example.com",
                                    "name": {"display_name": "Test User"}})
        if path == "/2/files/list_folder":
            return self._send(200, {"entries": direct_children(body.get("path", "")),
                                    "cursor": "CUR", "has_more": False})
        if path == "/2/files/list_folder/continue":
            return self._send(200, {"entries": [], "cursor": "CUR", "has_more": False})
        if path == "/2/files/get_metadata":
            p = body["path"]
            if p in FILES:
                return self._send(200, meta(p, False))
            if p in FOLDERS:
                return self._send(200, meta(p, True))
            return self._send(409, {"error_summary": "path/not_found/"})
        if path == "/2/files/create_folder_v2":
            FOLDERS.add(body["path"])
            return self._send(200, {"metadata": meta(body["path"], True)})
        if path == "/2/files/move_v2":
            src, dst = body["from_path"], body["to_path"]
            if src in FILES:
                FILES[dst] = FILES.pop(src)
            elif src in FOLDERS:
                FOLDERS.discard(src)
                FOLDERS.add(dst)
            return self._send(200, {"metadata": meta(dst, dst in FOLDERS)})
        if path == "/2/files/delete_v2":
            p = body["path"]
            was_dir = p in FOLDERS
            for f in list(FILES):
                if f == p or f.startswith(p + "/"):
                    FILES.pop(f, None)
            for d in list(FOLDERS):
                if d == p or d.startswith(p + "/"):
                    FOLDERS.discard(d)
            return self._send(200, {"metadata": meta(p, was_dir)})

        self._send(409, {"error_summary": "unknown_endpoint/"})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8820
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
