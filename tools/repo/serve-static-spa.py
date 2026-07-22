#!/usr/bin/env python3
import argparse
import http.server
import os
import socketserver


class SpaHandler(http.server.SimpleHTTPRequestHandler):
    def translate_path(self, path):
        translated = super().translate_path(path)
        if os.path.exists(translated):
            return translated

        if path.startswith("/_"):
            return translated

        basename = os.path.basename(path)
        if "." in basename:
            return translated

        return os.path.join(self.directory, "index.html")


class ThreadingSpaServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser(description="Serve a static SPA with index.html fallback.")
    parser.add_argument("directory")
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", default=8080, type=int)
    args = parser.parse_args()

    handler = lambda *handler_args, **handler_kwargs: SpaHandler(
        *handler_args,
        directory=args.directory,
        **handler_kwargs,
    )
    with ThreadingSpaServer((args.bind, args.port), handler) as server:
        print(f"serving {args.directory} on http://{args.bind}:{args.port}")
        server.serve_forever()


if __name__ == "__main__":
    main()
