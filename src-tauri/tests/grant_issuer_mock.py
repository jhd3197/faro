#!/usr/bin/env python3
"""Reference issuer for the Faro Grant Protocol v1 (docs/grant-links.md).

A minimal, stdlib-only implementation of the two endpoints any issuer must
serve, useful both as a test fixture and as sample code for third-party
issuers (hosting panels, ServerKit-style extensions, ...):

    GET  /.well-known/faro-grant/{token}       -> the grant manifest
    POST /.well-known/faro-grant/{token}/key   -> install the public key

A real issuer would store HASHED tokens with a TTL + one-time redemption,
rate-limit both endpoints, and append the uploaded key to the grant user's
~/.ssh/authorized_keys on each server (and jump host). This mock keeps one
fixed grant for token "test-token-123456" in memory and simply records every
posted key — the flow, not the fleet plumbing, is what matters here.

Run:  python src-tauri/tests/grant_issuer_mock.py [--port 9321]
"""
import argparse
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

# The one grant this mock knows about. Tokens in production: secrets.token_urlsafe(24).
VALID_TOKEN = "test-token-123456"

MANIFEST = {
    "version": 1,
    "issuer": "Faro Mock Issuer",
    "name": "Demo grant — 2 servers",
    "group": "Mock Issuer / Demo",
    "expires_at": "2026-08-07T00:00:00Z",
    "auth": {"type": "key-install"},
    "connections": [
        {
            "name": "web-1",
            "protocol": "sftp",
            "host": "10.0.0.11",
            "port": 22,
            "username": "deploy",
            "path": "/var/www",
            "jump": {
                "host": "bastion.example.com",
                "port": 22,
                "username": "faro-grant",
            },
        },
        {
            "name": "web-2",
            "protocol": "sftp",
            "host": "10.0.0.12",
            "port": 22,
            "username": "deploy",
            "path": "/srv/app",
        },
    ],
}

# Every public key posted to /key, in order — tests assert on this.
POSTED_KEYS = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _token(self, suffix=""):
        """Extract and check the token from the request path, or 404."""
        prefix = "/.well-known/faro-grant/"
        path = self.path
        if not path.startswith(prefix) or not path.endswith(suffix):
            self._send(404, {"error": "not found"})
            return None
        token = path[len(prefix):len(path) - len(suffix) if suffix else None]
        if token != VALID_TOKEN:
            # Unknown / redeemed / expired tokens all look the same to a caller.
            self._send(404, {"error": "unknown, redeemed, or expired token"})
            return None
        return token

    def do_GET(self):
        if self._token() is None:
            return
        self._send(200, MANIFEST)

    def do_POST(self):
        if self._token(suffix="/key") is None:
            return
        n = int(self.headers.get("Content-Length", 0))
        try:
            payload = json.loads(self.rfile.read(n) or b"{}")
        except json.JSONDecodeError:
            self._send(400, {"error": "invalid JSON body"})
            return
        public_key = payload.get("public_key", "")
        # The only credential material in the protocol: one OpenSSH public-key line.
        if not public_key.startswith("ssh-ed25519 "):
            self._send(400, {"error": "public_key must be an ssh-ed25519 authorized_keys line"})
            return
        POSTED_KEYS.append(public_key)
        # A real issuer installs the key on every listed server (+ jump host)
        # here and reports per-server failures. The mock "installs" everywhere.
        installed = [c.get("name") or c["host"] for c in MANIFEST["connections"]]
        self._send(200, {"installed": installed, "failed": []})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=9321)
    args = ap.parse_args()
    HTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
