#!/usr/bin/env python3

import argparse
import shutil
from pathlib import Path


TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux-x64", "libprns_host.so"),
    "aarch64-unknown-linux-gnu": ("linux-arm64", "libprns_host.so"),
    "x86_64-unknown-linux-musl": ("linux-musl-x64", "libprns_host.so"),
    "aarch64-unknown-linux-musl": ("linux-musl-arm64", "libprns_host.so"),
    "x86_64-apple-darwin": ("osx-x64", "libprns_host.dylib"),
    "aarch64-apple-darwin": ("osx-arm64", "libprns_host.dylib"),
    "x86_64-pc-windows-msvc": ("win-x64", "prns_host.dll"),
    "aarch64-pc-windows-msvc": ("win-arm64", "prns_host.dll"),
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    artifacts = Path(args.artifacts).resolve()
    output = Path(args.output).resolve()
    for target, (runtime, library) in TARGETS.items():
        matches = sorted(
            path
            for path in artifacts.rglob(library)
            if target in path.parts
            or f"host-native-{target}" in path.parts
        )
        if len(matches) != 1:
            raise SystemExit(
                f"expected one {library} for {target}, found {len(matches)}"
            )
        destination = output / runtime / "native" / library
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(matches[0], destination)


if __name__ == "__main__":
    main()
