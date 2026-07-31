#!/usr/bin/env python3
"""A minimal in-memory mock of the Shopify Admin REST API (themes + theme
assets), used by the `live_shopify_roundtrip` integration test in
`src/remotefs/shopify.rs`.

It is NOT a faithful Shopify — just enough of the shapes Faro's ShopifySession
/ ShopifyFs actually call, so the whole client path (client-credentials token
exchange, static-token passthrough, 429 Retry-After retry, theme list, asset
list/get/put/delete) can be exercised without a real store.

Run:  python src-tauri/tests/shopify_mock.py <port>
Endpoints (one host serves the API base + the token URL):
  GET    /themes.json
  GET    /themes/{id}/assets.json                     flat asset listing
  GET    /themes/{id}/assets.json?asset[key]={key}    one asset (value|attachment)
  PUT    /themes/{id}/assets.json                     { asset: { key, value|attachment } }
  DELETE /themes/{id}/assets.json?asset[key]={key}
  POST   /admin/oauth/access_token                    grant_type=client_credentials
Auth: API calls need `X-Shopify-Access-Token: shpat_test|CC-TOKEN`, else 401.
429 hook: the first PUT whose key contains "rl429" fails once with
`429 + Retry-After: 0.1`, then succeeds on the client's retry.
"""
import base64
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

VALID_TOKENS = {"shpat_test", "CC-TOKEN"}

THEMES = [
    {"id": 1, "name": "Dawn", "role": "main"},
    {"id": 2, "name": "Draft", "role": "unpublished"},
]

TEXT_EXTS = ("liquid", "json", "css", "js", "svg", "txt", "md", "html", "xml", "csv")


def is_text(key):
    name = key.rsplit("/", 1)[-1]
    if name == ".faro-keep":
        return True
    return name.rsplit(".", 1)[-1].lower() in TEXT_EXTS


# theme_id -> {key: bytes}
ASSETS = {
    1: {
        "layout/theme.liquid": b"<html>main</html>",
        "templates/index.json": b"{}",
    },
    2: {
        "layout/theme.liquid": b"<html>draft</html>",
        "templates/index.json": b"{}",
        "assets/base.css": b"body{}",
    },
}

RL429_SEEN = set()  # keys that already burned their one 429


def meta(key, data):
    return {
        "key": key,
        "size": len(data),
        "updated_at": "2024-07-15T09:30:00-04:00",
        "content_type": "text/plain" if is_text(key) else "image/png",
    }


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj, headers=None):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _authed(self):
        if self.headers.get("X-Shopify-Access-Token", "") not in VALID_TOKENS:
            self._send(401, {"errors": "Unauthorized"})
            return False
        return True

    def _route(self):
        u = urlparse(self.path)
        m = re.match(r"^/themes/(\d+)/assets\.json$", u.path)
        theme = int(m.group(1)) if m else None
        key = parse_qs(u.query).get("asset[key]", [None])[0]
        return u.path, theme, key

    def _maybe_429(self, key):
        """429-injection hook: keys containing "rl429" fail once."""
        if "rl429" in key and key not in RL429_SEEN:
            RL429_SEEN.add(key)
            self._send(429, {"errors": "Exceeded API rate limit"},
                       headers={"Retry-After": "0.1"})
            return True
        return False

    def do_GET(self):
        path, theme, key = self._route()
        if path == "/themes.json":
            if not self._authed():
                return
            return self._send(200, {"themes": THEMES})
        if theme is not None:
            if not self._authed():
                return
            files = ASSETS.setdefault(theme, {})
            if key is None:
                listing = [meta(k, v) for k, v in sorted(files.items())]
                return self._send(200, {"assets": listing})
            data = files.get(key)
            if data is None:
                return self._send(404, {"errors": "Not Found"})
            asset = meta(key, data)
            if is_text(key):
                asset["value"] = data.decode()
            else:
                asset["attachment"] = base64.b64encode(data).decode()
            return self._send(200, {"asset": asset})
        self._send(404, {"errors": "Not Found"})

    def do_PUT(self):
        _, theme, _ = self._route()
        if theme is None:
            return self._send(404, {"errors": "Not Found"})
        if not self._authed():
            return
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n).decode() or "{}")
        asset = body.get("asset", {})
        key = asset.get("key", "")
        if not key:
            return self._send(422, {"errors": "key can't be blank"})
        if self._maybe_429(key):
            return
        if "value" in asset:
            data = asset["value"].encode()
        else:
            data = base64.b64decode(asset.get("attachment", ""))
        ASSETS.setdefault(theme, {})[key] = data
        self._send(200, {"asset": meta(key, data)})

    def do_DELETE(self):
        _, theme, key = self._route()
        if theme is None or key is None:
            return self._send(404, {"errors": "Not Found"})
        if not self._authed():
            return
        data = ASSETS.setdefault(theme, {}).pop(key, None)
        if data is None:
            return self._send(404, {"errors": "Not Found"})
        self._send(200, {"asset": meta(key, data)})

    def do_POST(self):
        if self.path == "/admin/oauth/access_token":
            n = int(self.headers.get("Content-Length", 0))
            body = json.loads(self.rfile.read(n).decode() or "{}")
            ok = (body.get("grant_type") == "client_credentials"
                  and body.get("client_id") and body.get("client_secret"))
            if ok:
                return self._send(200, {"access_token": "CC-TOKEN",
                                        "scope": "read_themes,write_themes",
                                        "expires_in": 86399})
            return self._send(401, {"errors": "invalid_client"})
        self._send(404, {"errors": "Not Found"})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8830
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
