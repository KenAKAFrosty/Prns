#!/usr/bin/env python3

import argparse
import shutil
from pathlib import Path


LIBRARIES = {
    "x86_64-unknown-linux-gnu": "libprns_host.so",
    "aarch64-unknown-linux-gnu": "libprns_host.so",
    "x86_64-unknown-linux-musl": "libprns_host.so",
    "aarch64-unknown-linux-musl": "libprns_host.so",
    "x86_64-apple-darwin": "libprns_host.dylib",
    "aarch64-apple-darwin": "libprns_host.dylib",
    "x86_64-pc-windows-msvc": "prns_host.dll",
    "aarch64-pc-windows-msvc": "prns_host.dll",
}
ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=sorted(LIBRARIES))
    parser.add_argument("--library", required=True)
    parser.add_argument("--package", required=True)
    args = parser.parse_args()
    source = Path(args.library).resolve()
    expected = LIBRARIES[args.target]
    if source.name != expected or not source.is_file():
        raise SystemExit(
            f"expected an existing {expected} for {args.target}, got {source}"
        )
    package = Path(args.package).resolve()
    native = package / "src" / "personal_rns" / "native"
    if native.exists():
        shutil.rmtree(native)
    native.mkdir(parents=True)
    shutil.copy2(source, native / expected)
    shutil.copy2(ROOT / "LICENSE-APACHE", package / "LICENSE-APACHE")
    shutil.copy2(ROOT / "LICENSE-MIT", package / "LICENSE-MIT")


if __name__ == "__main__":
    main()
