#!/usr/bin/env python3
"""A minimal in-memory mock of the HubSpot CMS Source Code API v3 (draft +
published environments) and the Files API v3 (the File Manager), used by the
`live_hubspot_roundtrip` integration test in `src/remotefs/hubspot.rs`.

It is NOT a faithful HubSpot — just enough of the shapes Faro's
HubSpotSession / HubSpotFs actually call, so the whole client path (Bearer
auth, the connect-time metadata probe, portal label, folder metadata walks,
content get/put/delete, the 429 Retry-After retry, and the extension
whitelist) can be exercised without a real portal. Design-Manager folders
materialize from file paths, like the real API (no empty directories). The
File Manager side keeps a real in-memory folder tree with paged listings
(page size 2, to force cursor loops), multipart upload/replace, PATCH
renames, signed URLs for PRIVATE files, async folder updates (task token +
status poll), and folder delete. The HubDB side serves three tables (paged
listings at 2, to force cursor loops): table metadata with columns, and
draft rows exercising text, numbers, nulls, missing keys, unicode, and
values with commas/quotes/newlines (to prove the client's CSV escaping).

Run:  python src-tauri/tests/hubspot_mock.py <port>
Endpoints (Source Code API):
  GET    /account-info/v3/details                       portal label probe
  GET    /cms/v3/source-code/{env}/metadata/{path}      file/folder metadata
  GET    /cms/v3/source-code/{env}/content/{path}       octet-stream download
  PUT    /cms/v3/source-code/{env}/content/{path}       multipart `file` field
  DELETE /cms/v3/source-code/{env}/content/{path}
Endpoints (Files API v3):
  GET    /files/v3/files/search?parentFolderId=&limit=&after=  paged listing
  POST   /files/v3/files                                multipart upload
  PUT    /files/v3/files/{id}                           multipart replace
  PATCH  /files/v3/files/{id}                           JSON {"name": stem}
  DELETE /files/v3/files/{id}
  GET    /files/v3/files/{id}/signed-url                {"url": ...} (PRIVATE)
  GET    /files/v3/folders/search?parentFolderId=&limit=&after=
  POST   /files/v3/folders                              JSON {name, parentFolderPath}
  PATCH  /files/v3/folders/{id}                         async: task token
  GET    /files/v3/folders/async/tasks/{taskId}/status
  DELETE /files/v3/folders/{id}                         (400 when non-empty)
  GET    /cdn{path}          public file bytes (PRIVATE files 404 here)
  GET    /cdn-signed/{id}?token=...                     signed-url target
Endpoints (HubDB API v3, read-only):
  GET    /cms/v3/hubdb/tables?limit=&after=             paged table listing
  GET    /cms/v3/hubdb/tables/{idOrName}/draft          table metadata+columns
  GET    /cms/v3/hubdb/tables/{idOrName}/rows/draft?limit=&after=  paged rows
Auth: API calls need `Authorization: Bearer pat-test`, else 401. The /cdn
paths are unauthenticated (they are the CDN, not the API).
400 hook: PUT with a non-whitelisted extension fails with 400.
429 hook: the first PUT whose path contains "rl429" fails once with
`429 + Retry-After: 0.1`, then succeeds on the client's retry.
"""
import json
import re
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, unquote, urlparse

VALID_TOKEN = "pat-test"
# A valid token whose private app lacks the `content` scope: every Source
# Code API call 403s the way HubSpot's MISSING_SCOPES does.
NO_CONTENT_TOKEN = "pat-no-content"

ALLOWED_EXTS = ("css", "js", "json", "html", "txt", "md", "jpg", "jpeg", "png",
                "gif", "map", "svg", "ttf", "woff", "woff2", "zip")

NOW_MS = 1721035800000

# environment -> {path: bytes}
FILES = {
    "draft": {
        "themes/example/main.css": b"body{}",
        "themes/example/theme.json": b"{}",
        "themes/example/templates/index.html": b"<html>draft</html>",
    },
    "published": {
        "themes/example/main.css": b"body{color:red}",
        "themes/example/templates/index.html": b"<html>live</html>",
    },
}

# environment -> {path: updated-ms}
MTIMES = {env: {p: NOW_MS for p in files} for env, files in FILES.items()}

RL429_SEEN = set()  # paths that already burned their one 429

# --- Files API v3 (File Manager) state --------------------------------------
# v3 timestamps are ISO-8601 strings; ids are strings (numeric tolerated).
ISO = "2024-07-15T09:30:00Z"

# Set in __main__ so defaultHostingUrl/signed urls can point back here.
BASE_URL = "http://127.0.0.1:8840"

# folder id -> folder (root folders have parentFolderId None)
FM_FOLDERS = {
    "100": {"id": "100", "name": "library", "path": "/library",
            "parentFolderId": None, "createdAt": ISO, "updatedAt": ISO},
    "101": {"id": "101", "name": "docs", "path": "/library/docs",
            "parentFolderId": "100", "createdAt": ISO, "updatedAt": ISO},
}

# file id -> file (`data` is the mock's byte store, never serialized)
FM_FILES = {
    "200": {"id": "200", "name": "root-note", "path": "/root-note.txt",
            "parentFolderId": None, "access": "PUBLIC_NOT_INDEXABLE",
            "createdAt": ISO, "updatedAt": ISO, "data": b"hello root"},
    "201": {"id": "201", "name": "logo", "path": "/library/logo.png",
            "parentFolderId": "100", "access": "PUBLIC_NOT_INDEXABLE",
            "createdAt": ISO, "updatedAt": ISO, "data": b"png-bytes"},
    "202": {"id": "202", "name": "secret", "path": "/library/docs/secret.pdf",
            "parentFolderId": "101", "access": "PRIVATE",
            "createdAt": ISO, "updatedAt": ISO, "data": b"pdf-bytes"},
}

FM_NEXT_ID = [300]
# Async folder-update tasks: task id -> {"polls", "op"}
FM_TASKS = {}
FM_PAGE_LIMIT = 2  # tiny server-side pages force the client's cursor loop

# --- HubDB API v3 state ------------------------------------------------------
# Three tables (the list pages at 2, forcing the client's cursor loop).
# `pricing` rows exercise text, numbers, nulls, missing keys, unicode, and
# comma/quote/newline values; `archive` is empty (header-only CSV).
HUBDB_TABLES = {
    "pricing": {
        "id": "7001", "name": "pricing", "label": "Pricing",
        "updatedAt": ISO,
        "columns": [
            {"name": "plan", "label": "Plan", "type": "TEXT"},
            {"name": "price", "label": "Price", "type": "NUMBER"},
            {"name": "notes", "label": "Notes", "type": "TEXT"},
        ],
        "rows": [
            {"id": "101", "values": {"plan": "Starter", "price": 0,
                                     "notes": None}},
            {"id": "102", "values": {"plan": "Pro, annual", "price": 49.5,
                                     "notes": "said \"hi\"\nthen left"}},
            {"id": "103", "values": {"plan": "Café plan ☕", "price": 500}},
        ],
    },
    "team": {
        "id": "7002", "name": "team", "label": "Team directory",
        "updatedAt": ISO,
        "columns": [
            {"name": "full_name", "label": "Name", "type": "TEXT"},
            {"name": "active", "label": "Active", "type": "BOOLEAN"},
        ],
        "rows": [
            {"id": "201", "values": {"full_name": "Ada", "active": True}},
        ],
    },
    "archive": {
        "id": "7003", "name": "archive", "label": "Archive",
        "updatedAt": ISO,
        "columns": [
            {"name": "archived", "label": "Archived", "type": "BOOLEAN"},
        ],
        "rows": [],
    },
}


def fm_file_json(f):
    """The wire shape of one file (no `data`; signed urls for PRIVATE)."""
    out = {k: v for k, v in f.items() if k != "data"}
    out["size"] = len(f["data"])
    out["defaultHostingUrl"] = BASE_URL + "/cdn" + f["path"]
    out["url"] = out["defaultHostingUrl"]
    return out


def fm_folder_by_path(path):
    path = "/" + path.strip("/") if path.strip("/") else None
    for f in FM_FOLDERS.values():
        if f["path"] == path:
            return f
    return None


def fm_paged(items, query, page_cap=None):
    """HubSpot v3 paging: `limit` + `after` cursor -> results + paging.
    `page_cap` simulates a small server-side max page size (the HubDB mock
    endpoints cap at 2, to force the client's cursor loop)."""
    items = sorted(items, key=lambda x: int(x["id"]))
    try:
        limit = int(query.get("limit", ["100"])[0])
    except ValueError:
        limit = 100
    if page_cap is not None:
        limit = min(limit, page_cap)
    start = 0
    if "after" in query:
        try:
            start = int(query["after"][0])
        except ValueError:
            start = 0
    page = items[start:start + limit]
    body = {"results": page}
    if start + limit < len(items):
        body["paging"] = {"next": {"after": str(start + limit)}}
    return body


def allowed_ext(path):
    name = path.rsplit("/", 1)[-1]
    return "." in name and name.rsplit(".", 1)[-1].lower() in ALLOWED_EXTS


def folders(files):
    """Every folder prefix materialized from the file paths (the real API has
    no empty folders either — they exist only while a file sits under them)."""
    out = set()
    for p in files:
        parts = p.split("/")
        for i in range(1, len(parts)):
            out.add("/".join(parts[:i]))
    return out


def metadata(env, path):
    """Source Code API metadata for `path`, or None when it doesn't exist.
    A folder's `children` array lists its immediate files + folders."""
    files = FILES[env]
    mtimes = MTIMES[env]
    if path in files:
        return {
            "name": path.rsplit("/", 1)[-1],
            "folder": False,
            "size": len(files[path]),
            "createdAt": NOW_MS,
            "updatedAt": mtimes.get(path, NOW_MS),
        }
    if path != "" and path not in folders(files):
        return None
    prefix = f"{path}/" if path else ""
    children = []
    seen_dirs = set()
    for p in sorted(files):
        if not p.startswith(prefix):
            continue
        rest = p[len(prefix):]
        head, _, tail = rest.partition("/")
        if tail:  # nested deeper: a folder child
            if head not in seen_dirs:
                seen_dirs.add(head)
                children.append({
                    "name": head,
                    "folder": True,
                    "createdAt": NOW_MS,
                    "updatedAt": NOW_MS,
                })
        else:
            children.append({
                "name": head,
                "folder": False,
                "size": len(files[p]),
                "createdAt": NOW_MS,
                "updatedAt": mtimes.get(p, NOW_MS),
            })
    return {
        "name": path.rsplit("/", 1)[-1] if path else "",
        "folder": True,
        "createdAt": NOW_MS,
        "updatedAt": NOW_MS,
        "children": children,
    }


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj=None, headers=None, raw=None):
        body = raw if raw is not None else json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type",
                         "application/octet-stream" if raw is not None
                         else "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _token(self):
        auth = self.headers.get("Authorization", "")
        return auth[len("Bearer "):] if auth.startswith("Bearer ") else ""

    def _authed(self):
        if self._token() not in (VALID_TOKEN, NO_CONTENT_TOKEN):
            self._send(401, {"status": "error", "message": "Unauthorized"})
            return False
        return True

    def _scope_blocked(self):
        """Source Code API gate: the NO_CONTENT_TOKEN app lacks `content`."""
        if self._token() == NO_CONTENT_TOKEN:
            self._send(403, {"status": "error", "category": "MISSING_SCOPES",
                             "message": "missing scope: content"})
            return True
        return False

    def _route(self):
        """`/cms/v3/source-code/{env}/{metadata|content}/{path}` ->
        (env, kind, path), else None."""
        u = urlparse(self.path)
        m = re.match(
            r"^/cms/v3/source-code/(draft|published)/(metadata|content)(?:/(.*))?$",
            u.path)
        if not m:
            return None
        env, kind, path = m.group(1), m.group(2), unquote(m.group(3) or "")
        # The design root arrives double-encoded as %252F (the live edge
        # rejects single-encoded %2F and empty segments with a bare 404);
        # one unquote above leaves "%2F", which is the root.
        if path == "%2F":
            path = ""
        return env, kind, path.rstrip("/")

    def _parse_multipart_all(self):
        """Split a multipart/form-data body into (text fields, file part)."""
        ctype = self.headers.get("Content-Type", "")
        m = re.search(r"boundary=([^;]+)", ctype)
        if not m:
            return None
        boundary = m.group(1).strip('"').encode()
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        fields = {}
        file_part = None
        for part in body.split(b"--" + boundary):
            header, _, data = part.partition(b"\r\n\r\n")
            if not data:
                continue
            if data.endswith(b"\r\n"):
                data = data[:-2]
            nm = re.search(rb'name="([^"]*)"', header)
            if not nm:
                continue
            name = nm.group(1).decode()
            fm = re.search(rb'filename="([^"]*)"', header)
            if fm:
                file_part = (fm.group(1).decode(), data)
            else:
                fields[name] = data.decode("utf-8", "replace")
        return fields, file_part

    def _parse_multipart(self):
        """Pull the `file` field out of a multipart/form-data body."""
        parsed = self._parse_multipart_all()
        return parsed[1] if parsed else None

    def _files_route(self):
        """`/files/v3/(files|folders)(/{rest})` -> (kind, rest), else None."""
        u = urlparse(self.path)
        m = re.match(r"^/files/v3/(files|folders)(?:/(.*))?$", u.path)
        if not m:
            return None
        return m.group(1), (m.group(2) or "").strip("/")

    def _hubdb_route(self):
        """`/cms/v3/hubdb/tables(/{rest})` -> rest ("" for the list), else
        None."""
        u = urlparse(self.path)
        m = re.match(r"^/cms/v3/hubdb/tables(?:/(.*))?$", u.path)
        if not m:
            return None
        return (m.group(1) or "").strip("/")

    def _query(self):
        return parse_qs(urlparse(self.path).query)

    # ---- Files API v3 handlers ----

    def _fm_list(self, kind):
        q = self._query()
        parent = q.get("parentFolderId", [None])[0]
        store = FM_FOLDERS if kind == "folders" else FM_FILES
        if parent is None:
            # Real v3 behavior: no parentFolderId lists EVERYTHING (the
            # client filters root children by path depth itself).
            items = list(store.values())
        else:
            items = [x for x in store.values() if x.get("parentFolderId") == parent]
        if kind == "files":
            items = [fm_file_json(f) for f in items]
        self._send(200, fm_paged(items, q))

    def _fm_upload(self):
        parsed = self._parse_multipart_all()
        if parsed is None or parsed[1] is None:
            return self._send(400, {"status": "error",
                                    "message": "missing multipart `file` field"})
        fields, (_, data) = parsed
        name = fields.get("fileName") or "file"
        folder_path = fields.get("folderPath", "/")
        try:
            options = json.loads(fields.get("options", "{}"))
        except json.JSONDecodeError:
            options = {}
        if "access" not in options:
            return self._send(400, {"status": "error",
                                    "message": "options.access is required"})
        folder = fm_folder_by_path(folder_path)
        if folder_path.strip("/") and folder is None:
            return self._send(404, {"status": "error",
                                    "message": f"folder not found: {folder_path}"})
        parent_id = folder["id"] if folder else None
        parent_path = folder["path"] if folder else ""
        path = f"{parent_path}/{name}"
        if any(f["path"] == path for f in FM_FILES.values()):
            return self._send(409, {"status": "error",
                                    "message": f"file already exists: {path}"})
        fid = str(FM_NEXT_ID[0])
        FM_NEXT_ID[0] += 1
        FM_FILES[fid] = {
            "id": fid, "name": name.rsplit(".", 1)[0] if "." in name else name,
            "path": path, "parentFolderId": parent_id,
            "access": options["access"],
            "createdAt": ISO, "updatedAt": ISO, "data": data,
        }
        self._send(201, fm_file_json(FM_FILES[fid]))

    def _fm_replace(self, fid):
        f = FM_FILES.get(fid)
        if f is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        parsed = self._parse_multipart_all()
        if parsed is None or parsed[1] is None:
            return self._send(400, {"status": "error",
                                    "message": "missing multipart `file` field"})
        f["data"] = parsed[1][1]
        f["updatedAt"] = ISO
        self._send(200, fm_file_json(f))

    def _fm_rename_file(self, fid):
        f = FM_FILES.get(fid)
        if f is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        body = self._json_body()
        stem = body.get("name")
        if not stem:
            return self._send(400, {"status": "error", "message": "Bad Request"})
        # The API's `name` is extensionless — the path keeps the old ext.
        old_base = f["path"].rsplit("/", 1)[-1]
        ext = "." + old_base.rsplit(".", 1)[-1] if "." in old_base else ""
        parent = f["path"].rsplit("/", 1)[0]
        f["name"] = stem
        f["path"] = f"{parent}/{stem}{ext}"
        f["updatedAt"] = ISO
        self._send(200, fm_file_json(f))

    def _fm_signed_url(self, fid):
        f = FM_FILES.get(fid)
        if f is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        body = fm_file_json(f)
        body["url"] = f"{BASE_URL}/cdn-signed/{fid}?token=sig-{fid}"
        self._send(200, body)

    def _fm_create_folder(self):
        body = self._json_body()
        name = body.get("name")
        parent_path = body.get("parentFolderPath", "/")
        if not name:
            return self._send(400, {"status": "error", "message": "Bad Request"})
        parent = fm_folder_by_path(parent_path)
        if parent_path.strip("/") and parent is None:
            return self._send(404, {"status": "error",
                                    "message": f"folder not found: {parent_path}"})
        parent_id = parent["id"] if parent else None
        base = parent["path"] if parent else ""
        fid = str(FM_NEXT_ID[0])
        FM_NEXT_ID[0] += 1
        FM_FOLDERS[fid] = {"id": fid, "name": name, "path": f"{base}/{name}",
                           "parentFolderId": parent_id,
                           "createdAt": ISO, "updatedAt": ISO}
        self._send(201, FM_FOLDERS[fid])

    def _fm_update_folder(self, fid):
        """Folder updates are ASYNC in v3: answer 202 + task token; the task
        goes PROCESSING on the first status poll, applies on the second."""
        folder = FM_FOLDERS.get(fid)
        if folder is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        body = self._json_body()
        task_id = str(FM_NEXT_ID[0])
        FM_NEXT_ID[0] += 1
        FM_TASKS[task_id] = {"polls": 0,
                             "op": ("rename_folder", fid, body.get("name"))}
        self._send(202, {"id": task_id, "links": {
            "status": f"/files/v3/folders/async/tasks/{task_id}/status"}})

    def _fm_task_status(self, task_id):
        task = FM_TASKS.get(task_id)
        if task is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        task["polls"] += 1
        if task["polls"] < 2:
            return self._send(200, {"status": "PROCESSING", "taskId": task_id})
        op, fid, new_name = task["op"]
        if op == "rename_folder" and new_name:
            folder = FM_FOLDERS.get(fid)
            if folder is not None:
                old_path = folder["path"]
                parent = old_path.rsplit("/", 1)[0]
                new_path = f"{parent}/{new_name}"
                folder["name"] = new_name
                folder["path"] = new_path
                folder["updatedAt"] = ISO
                # Descendants move with it (files and subfolders).
                for f in FM_FOLDERS.values():
                    if f["path"].startswith(old_path + "/"):
                        f["path"] = new_path + f["path"][len(old_path):]
                for f in FM_FILES.values():
                    if f["path"].startswith(old_path + "/"):
                        f["path"] = new_path + f["path"][len(old_path):]
        del FM_TASKS[task_id]
        self._send(200, {"status": "COMPLETE", "taskId": task_id})

    def _fm_delete_folder(self, fid):
        folder = FM_FOLDERS.pop(fid, None)
        if folder is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        if any(f.get("parentFolderId") == fid for f in FM_FOLDERS.values()) or \
           any(f.get("parentFolderId") == fid for f in FM_FILES.values()):
            FM_FOLDERS[fid] = folder  # put it back
            return self._send(400, {"status": "error",
                                    "message": "folder is not empty"})
        self._send(204)

    def _json_body(self):
        n = int(self.headers.get("Content-Length", 0))
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except json.JSONDecodeError:
            return {}

    # ---- HubDB API v3 handlers (read-only) ----

    def _hubdb_table(self, id_or_name):
        key = unquote(id_or_name)
        for t in HUBDB_TABLES.values():
            if t["id"] == key or t["name"] == key:
                return t
        return None

    def _hubdb_get(self, rest):
        q = self._query()
        if not rest:
            # Table listing: summaries only (no columns/rows), paged at 2.
            summaries = [{"id": t["id"], "name": t["name"], "label": t["label"],
                          "updatedAt": t["updatedAt"]}
                         for t in HUBDB_TABLES.values()]
            return self._send(200, fm_paged(summaries, q, page_cap=FM_PAGE_LIMIT))
        if rest.endswith("/rows/draft"):
            t = self._hubdb_table(rest[:-len("/rows/draft")])
            if t is None:
                return self._send(404, {"status": "error", "message": "Not Found"})
            return self._send(200, fm_paged(list(t["rows"]), q,
                                            page_cap=FM_PAGE_LIMIT))
        if rest.endswith("/draft"):
            t = self._hubdb_table(rest[:-len("/draft")])
            if t is None:
                return self._send(404, {"status": "error", "message": "Not Found"})
            detail = {k: v for k, v in t.items() if k != "rows"}
            return self._send(200, detail)
        self._send(404, {"status": "error", "message": "Not Found"})

    def _fm_cdn(self, signed):
        """Unauthenticated CDN leg: /cdn{path} serves PUBLIC files (PRIVATE
        404s there); /cdn-signed/{id} serves anything with a token."""
        u = urlparse(self.path)
        if signed:
            m = re.match(r"^/cdn-signed/(\d+)$", u.path)
            q = self._query()
            f = FM_FILES.get(m.group(1)) if m else None
            if f is None or not q.get("token", [""])[0].startswith("sig-"):
                return self._send(403, {"status": "error", "message": "Forbidden"})
            return self._send(200, raw=f["data"])
        path = unquote(u.path[len("/cdn"):])
        for f in FM_FILES.values():
            if f["path"] == path:
                if f["access"] == "PRIVATE":
                    break
                return self._send(200, raw=f["data"])
        self._send(404, {"status": "error", "message": "Not Found"})

    def do_GET(self):
        if self.path == "/account-info/v3/details":
            if not self._authed():
                return
            return self._send(200, {"portalId": 987654321,
                                    "accountType": "STANDARD",
                                    "portalDomain": "mock.hubspot.com"})
        if self.path.startswith("/cdn"):
            return self._fm_cdn(signed=self.path.startswith("/cdn-signed"))
        hrest = self._hubdb_route()
        if hrest is not None:
            if not self._authed():
                return
            return self._hubdb_get(hrest)
        froute = self._files_route()
        if froute is not None:
            if not self._authed():
                return
            kind, rest = froute
            if not rest or rest == "search":
                return self._fm_list(kind)
            if kind == "files" and rest.endswith("/signed-url"):
                return self._fm_signed_url(rest.split("/")[0])
            if kind == "folders" and rest.startswith("async/tasks/"):
                parts = rest.split("/")
                if len(parts) == 4 and parts[3] == "status":
                    return self._fm_task_status(parts[2])
                return self._send(404, {"status": "error", "message": "Not Found"})
            return self._send(404, {"status": "error", "message": "Not Found"})
        route = self._route()
        if route is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        env, kind, path = route
        if not self._authed():
            return
        if self._scope_blocked():
            return
        if kind == "metadata":
            node = metadata(env, path)
            if node is None:
                return self._send(404, {"status": "error", "message": "Not Found"})
            return self._send(200, node)
        data = FILES[env].get(path)
        if data is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        self._send(200, raw=data)

    def do_POST(self):
        froute = self._files_route()
        if froute is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        if not self._authed():
            return
        kind, rest = froute
        if kind == "files" and not rest:
            return self._fm_upload()
        if kind == "folders" and not rest:
            return self._fm_create_folder()
        self._send(404, {"status": "error", "message": "Not Found"})

    def do_PATCH(self):
        froute = self._files_route()
        if froute is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        if not self._authed():
            return
        kind, rest = froute
        if kind == "files" and rest and "/" not in rest:
            return self._fm_rename_file(rest)
        if kind == "folders" and rest and "/" not in rest:
            return self._fm_update_folder(rest)
        self._send(404, {"status": "error", "message": "Not Found"})

    def do_PUT(self):
        froute = self._files_route()
        if froute is not None:
            if not self._authed():
                return
            kind, rest = froute
            if kind == "files" and rest and "/" not in rest:
                return self._fm_replace(rest)
            return self._send(404, {"status": "error", "message": "Not Found"})
        route = self._route()
        if route is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        env, kind, path = route
        if not self._authed():
            return
        if kind != "content" or not path:
            return self._send(400, {"status": "error", "message": "Bad Request"})
        # 429-injection hook: paths containing "rl429" fail once.
        if "rl429" in path and path not in RL429_SEEN:
            RL429_SEEN.add(path)
            return self._send(429, {"status": "error", "message": "rate limited"},
                              headers={"Retry-After": "0.1"})
        if not allowed_ext(path):
            return self._send(400, {"status": "error",
                                    "message": f"file type not allowed: {path}"})
        parsed = self._parse_multipart()
        if parsed is None:
            return self._send(400, {"status": "error",
                                    "message": "missing multipart `file` field"})
        _, data = parsed
        FILES[env][path] = data
        MTIMES[env][path] = int(time.time() * 1000)
        self._send(200, metadata(env, path))

    def do_DELETE(self):
        froute = self._files_route()
        if froute is not None:
            if not self._authed():
                return
            kind, rest = froute
            if kind == "files" and rest and "/" not in rest:
                f = FM_FILES.pop(rest, None)
                if f is None:
                    return self._send(404, {"status": "error",
                                            "message": "Not Found"})
                return self._send(204)
            if kind == "folders" and rest and "/" not in rest:
                return self._fm_delete_folder(rest)
            return self._send(404, {"status": "error", "message": "Not Found"})
        route = self._route()
        if route is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        env, kind, path = route
        if not self._authed():
            return
        if kind != "content":
            return self._send(404, {"status": "error", "message": "Not Found"})
        data = FILES[env].pop(path, None)
        MTIMES[env].pop(path, None)
        if data is None:
            return self._send(404, {"status": "error", "message": "Not Found"})
        self._send(200, {"status": "ok"})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8840
    BASE_URL = f"http://127.0.0.1:{port}"
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
