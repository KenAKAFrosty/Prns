#!/usr/bin/env python3
"""Build static Hopspot browser assets without changing a device or its web server."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def capture(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def port(value):
    parsed = int(value)
    if not 1 <= parsed <= 65535:
        raise argparse.ArgumentTypeError("WebSocket port must be between 1 and 65535")
    return parsed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="New bundle directory")
    parser.add_argument("--websocket-port", required=True, type=port,
                        help="Existing listener port on the same host; does not start a listener")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("output already exists; choose a new bundle directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hopspot-browser-", dir=output.parent) as temporary:
        bundle = Path(temporary) / "bundle"
        public = bundle / "public"
        subprocess.run([sys.executable, str(ROOT / "tools/prns"), "run",
                        "build.wasm-docs.stage", "--", str(public)], cwd=ROOT, check=True)
        index = public / "index.html"
        html = index.read_text()
        if html.count("</head>") != 1:
            raise ValueError("expected one browser document head")
        html = html.replace("</head>", f'<meta name="prns-websocket-port" content="{args.websocket_port}" />\n  </head>')
        index.write_text(html)
        (public / "assets").mkdir(exist_ok=True)
        shutil.copy2(ROOT / "docs/website/public/assets/favicon.svg", public / "assets/favicon.svg")
        for name in ("LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md"):
            shutil.copy2(ROOT / name, bundle / name)
        docs = bundle / "docs"
        (docs / "qualification").mkdir(parents=True)
        shutil.copy2(ROOT / "personal-hopspot/headless/docs/browser-hosting.md", docs / "browser-hosting.md")
        for name in ("browser-local-hosting-2026-09-29.md", "browser-local-hosting-2026-09-29.json"):
            shutil.copy2(ROOT / "personal-hopspot/headless/docs/qualification" / name,
                         docs / "qualification" / name)
        (bundle / "INSTALL.md").write_text(
            "# Hopspot browser development bundle\n\n"
            "Read the [local hosting guide](docs/browser-hosting.md). Serve only `public/`; "
            "keep node identity and state outside that directory. "
            "This bundle does not install a persistent service.\n"
        )
        required = ["index.html", "main.js", "sdk/browser/index.js", "sdk/browser/worker.js",
                    "pkg/prns_wasm.js", "pkg/prns_wasm_bg.wasm"]
        if any(not (public / name).is_file() for name in required):
            raise ValueError("incomplete browser output")
        assets = sorted(path for path in public.rglob("*") if path.is_file())
        if any(path.is_symlink() for path in public.rglob("*")):
            raise ValueError("browser assets must not contain symlinks")
        metadata = {
            "schema": 1,
            "artifact_kind": "hopspot-browser-development-bundle",
            "source_commit": capture("git", "rev-parse", "HEAD"),
            "working_tree_dirty": bool(capture("git", "status", "--porcelain")),
            "websocket_port": args.websocket_port,
            "public_bytes": sum(path.stat().st_size for path in assets),
            "public_files": len(assets),
            "wasm_bindgen": capture("wasm-bindgen", "--version"),
            "qualification": "Unsigned static assets; not firmware or a persistent installer",
        }
        (bundle / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
        files = sorted(path for path in bundle.rglob("*") if path.is_file())
        (bundle / "SHA256SUMS").write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(bundle).as_posix()}\n"
            for path in files
        ))
        bundle.rename(output)
    print(f"Hopspot browser development bundle: {output}")


if __name__ == "__main__":
    main()
