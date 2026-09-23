#!/usr/bin/env python3
"""Dependency-free static server for the Flappy Rust web build.

Why not `python -m http.server`?

* It does not reliably send ``Content-Type: application/wasm``. Browsers refuse
  to stream-compile a WebAssembly module served with the wrong MIME type, so the
  game would fail to load with a confusing error.
* It serves the whole working directory and always binds every interface, which
  is more exposure than needed for a local test.

Usage::

    python web/serve.py                      # http://127.0.0.1:8080/
    python web/serve.py --port 9000
    python web/serve.py --host 0.0.0.0       # reachable from a phone on the LAN

Only the ``web/`` directory is ever served, and path traversal outside it is
rejected.
"""

from __future__ import annotations

import argparse
import functools
import http.server
import mimetypes
import os
import socketserver
import sys

WEB_ROOT = os.path.dirname(os.path.abspath(__file__))


class Handler(http.server.SimpleHTTPRequestHandler):
    """Serves ``web/`` with correct WebAssembly MIME types and no caching."""

    def __init__(self, *args, directory: str = WEB_ROOT, **kwargs):
        super().__init__(*args, directory=directory, **kwargs)

    def guess_type(self, path):
        # Register explicitly: `mimetypes` does not know about .wasm on every
        # platform (notably older Python builds on Windows).
        if path.endswith(".wasm"):
            return "application/wasm"
        if path.endswith(".js"):
            return "text/javascript"
        return super().guess_type(path)

    def end_headers(self):
        # The build output is rewritten by scripts/build-web.*; a cached module
        # would silently serve the previous build.
        self.send_header("Cache-Control", "no-store, must-revalidate")
        # Needed if the page is ever put behind a cross-origin-isolated host.
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        super().end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write("  %s\n" % (fmt % args))


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="interface to bind (use 0.0.0.0 to test from a phone on the LAN)",
    )
    parser.add_argument("--port", type=int, default=8080, help="port to listen on")
    args = parser.parse_args()

    index = os.path.join(WEB_ROOT, "index.html")
    if not os.path.isfile(index):
        print(f"error: {index} is missing", file=sys.stderr)
        return 1
    if not os.path.isfile(os.path.join(WEB_ROOT, "pkg", "flappy_web.js")):
        print(
            "warning: web/pkg/ is missing or incomplete.\n"
            "         Run scripts/build-web.ps1 (or .sh) first.",
            file=sys.stderr,
        )

    mimetypes.add_type("application/wasm", ".wasm")
    handler = functools.partial(Handler, directory=WEB_ROOT)

    try:
        with Server((args.host, args.port), handler) as httpd:
            shown = "127.0.0.1" if args.host in ("0.0.0.0", "::") else args.host
            print(f"Flappy Rust is being served from {WEB_ROOT}")
            print(f"  open http://{shown}:{args.port}/")
            if args.host in ("0.0.0.0", "::"):
                print("  bound to all interfaces - reachable from other devices")
            print("  press Ctrl+C to stop")
            try:
                httpd.serve_forever()
            except KeyboardInterrupt:
                print("\nstopped")
    except OSError as err:
        print(f"error: could not bind {args.host}:{args.port}: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
