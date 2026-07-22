#!/usr/bin/env python3
"""Serve one extracted signed-candidate website on loopback for Web Serial testing."""

from __future__ import annotations

import argparse
from functools import partial
import http.server
from pathlib import Path, PurePosixPath
import sys
from urllib.parse import unquote, urlsplit


class CandidateHandler(http.server.SimpleHTTPRequestHandler):
    """Static handler with extensionless SPA fallback and release-safe headers."""

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Permissions-Policy", "serial=(self)")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def send_head(self):
        path = self.translate_path(self.path)
        if not Path(path).exists() and self.is_extensionless_route(self.path):
            original = self.path
            self.path = "/index.html"
            try:
                return super().send_head()
            finally:
                self.path = original
        return super().send_head()

    def list_directory(self, path):
        self.send_error(404, "File not found")
        return None

    def log_message(self, format, *arguments) -> None:
        # Qualification does not need access logs. Suppressing the raw request
        # target also prevents an accidentally pasted query value entering a log.
        return

    @staticmethod
    def is_extensionless_route(raw_url: str) -> bool:
        decoded = unquote(urlsplit(raw_url).path)
        if "\\" in decoded or any(part == ".." for part in decoded.split("/")):
            return False
        path = PurePosixPath(decoded)
        return not path.suffix


def validate_website_root(root: Path) -> Path:
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f"website root must be a real directory: {root}")
    resolved = root.resolve()
    if not (resolved / "index.html").is_file():
        raise ValueError(f"website root has no index.html: {root}")
    symlinks = [path.relative_to(resolved) for path in resolved.rglob("*") if path.is_symlink()]
    if symlinks:
        raise ValueError(f"website root contains symlinks: {symlinks}")
    return resolved


def create_server(root: Path, port: int) -> http.server.ThreadingHTTPServer:
    website = validate_website_root(root)
    handler = partial(CandidateHandler, directory=str(website))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    server.daemon_threads = True
    return server


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--website", type=Path, required=True)
    parser.add_argument("--port", type=int, default=8000)
    arguments = parser.parse_args()
    if not 1 <= arguments.port <= 65535:
        parser.error("--port must be between 1 and 65535")
    try:
        server = create_server(arguments.website, arguments.port)
    except (OSError, ValueError) as error:
        print(f"candidate server failed: {error}", file=sys.stderr)
        return 1
    address, port = server.server_address
    print(
        f"Serving the exact candidate on http://localhost:{port}/flash "
        f"(bound only to {address})",
        flush=True,
    )
    print("Press Ctrl-C to stop.", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped candidate server.")
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
