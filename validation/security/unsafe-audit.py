#!/usr/bin/env python3
"""Deterministic unsafe inventory for every shipped Cargo graph.

Unlike cargo-geiger, this scanner deliberately does not parse Rust syntax. It tokenizes Rust source
while discarding nested comments, cooked strings, raw strings, byte strings, and character literals,
then records every remaining `unsafe` token. Cargo metadata supplies the exact locked packages,
enabled features, sources, and release-graph membership.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Iterator

try:
    import tomllib
except ModuleNotFoundError:
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        raise SystemExit(
            "unsafe audit failed: Python 3.11+ (or the tomli package) is required"
        )


ROOT = Path(__file__).resolve().parents[2]
BASELINE = ROOT / "audits" / "unsafe-snapshot.json"
GRAPHS = (
    ("engine", "Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("host-c-linux", "prns-host/abi/c/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("host-c-macos", "prns-host/abi/c/Cargo.toml", "aarch64-apple-darwin"),
    ("host-c-windows", "prns-host/abi/c/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("daemon-linux", "prnsd/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("daemon-macos", "prnsd/Cargo.toml", "aarch64-apple-darwin"),
    ("daemon-windows", "prnsd/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("desktop-linux", "personal-hopspot/desktop/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("desktop-macos", "personal-hopspot/desktop/Cargo.toml", "aarch64-apple-darwin"),
    ("desktop-windows", "personal-hopspot/desktop/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("android", "personal-hopspot/mobile/android/rust/Cargo.toml", "aarch64-linux-android"),
    ("ios", "personal-hopspot/mobile/ios/rust/Cargo.toml", "aarch64-apple-ios"),
    ("nrf52840", "personal-hopspot/embedded/nrf52840/Cargo.toml", "thumbv7em-none-eabihf"),
    (
        "esp32-c6",
        "personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/Cargo.toml",
        "riscv32imac-unknown-none-elf",
    ),
    (
        "esp32-s3-heltec",
        "personal-hopspot/embedded/esp32/boards/heltec-v4/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    (
        "esp32-s3-heltec-r8",
        "personal-hopspot/embedded/esp32/boards/heltec-v4-r8/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    (
        "esp32-s3-tbeam",
        "personal-hopspot/embedded/esp32/boards/t-beam-supreme/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    ("wasm", "prns-wasm/Cargo.toml", "wasm32-unknown-unknown"),
)
UNSAFE_EXCEPTIONS = {
    "prns-core",
    "prns-ffi",
    "prns-runtime",
    "prns-runtime-embassy",
    "prns-host-c",
    "personal-hopspot-android",
    "personal-hopspot-ios",
    "t-echo",
    "personal-hopspot-esp32",
}
UNDOCUMENTED_UNSAFE_POLICY_EXCEPTIONS = {"prns-host-c"}
VENDOR_DIR_NAME = "vendor"


def run_metadata(manifest: str, target: str) -> dict:
    command = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
        "--manifest-path",
        str(ROOT / manifest),
    ]
    process = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if process.returncode:
        sys.stderr.write(process.stderr)
        raise RuntimeError(f"cargo metadata failed for {manifest} ({target})")
    return json.loads(process.stdout)


def raw_string_end(source: str, start: int) -> int | None:
    cursor = start
    if source.startswith("br", cursor):
        cursor += 2
    elif source.startswith("r", cursor):
        cursor += 1
    else:
        return None
    hashes = 0
    while cursor < len(source) and source[cursor] == "#":
        hashes += 1
        cursor += 1
    if cursor >= len(source) or source[cursor] != '"':
        return None
    marker = '"' + "#" * hashes
    end = source.find(marker, cursor + 1)
    return len(source) if end < 0 else end + len(marker)


def rust_tokens(source: str) -> Iterator[str]:
    cursor = 0
    length = len(source)
    while cursor < length:
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = length if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            depth = 1
            cursor += 2
            while cursor < length and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            continue
        raw_end = raw_string_end(source, cursor)
        if raw_end is not None:
            cursor = raw_end
            continue
        if source[cursor] == '"' or source.startswith('b"', cursor):
            cursor += 2 if source.startswith('b"', cursor) else 1
            while cursor < length:
                if source[cursor] == "\\":
                    cursor += 2
                elif source[cursor] == '"':
                    cursor += 1
                    break
                else:
                    cursor += 1
            continue
        char_start = cursor + 1 if source.startswith("b'", cursor) else cursor
        if char_start < length and source[char_start] == "'":
            look = char_start + 1
            if look < length and source[look] == "\\":
                look += 2
                while look < length and source[look] != "'":
                    look += 1
            elif look + 1 < length and source[look + 1] == "'":
                look += 1
            else:
                look = -1
            if look >= 0 and look < length and source[look] == "'":
                cursor = look + 1
                continue
        character = source[cursor]
        if character.isalpha() or character == "_":
            end = cursor + 1
            while end < length and (source[end].isalnum() or source[end] == "_"):
                end += 1
            yield source[cursor:end]
            cursor = end
            continue
        if character in "{}!":
            yield character
        cursor += 1


def unsafe_counts(package_root: Path) -> dict[str, int]:
    counts = {"tokens": 0, "blocks": 0, "functions": 0, "impls": 0, "externs": 0}
    for path in sorted(package_root.rglob("*.rs")):
        if "target" in path.parts or ".git" in path.parts:
            continue
        try:
            tokens = list(rust_tokens(path.read_text(encoding="utf-8")))
        except (OSError, UnicodeDecodeError):
            continue
        for index, token in enumerate(tokens):
            if token != "unsafe":
                continue
            counts["tokens"] += 1
            following = tokens[index + 1] if index + 1 < len(tokens) else ""
            if following == "fn":
                counts["functions"] += 1
            elif following == "impl":
                counts["impls"] += 1
            elif following == "extern":
                counts["externs"] += 1
            elif following == "{":
                counts["blocks"] += 1
    return counts


def display_source(package: dict, manifest_path: Path) -> str:
    if package.get("source"):
        return package["source"]
    try:
        return manifest_path.parent.relative_to(ROOT).as_posix() or "."
    except ValueError:
        return "path+external"


def assert_first_party_policy(package: dict, manifest_path: Path) -> None:
    try:
        relative_manifest = manifest_path.relative_to(ROOT)
    except ValueError:
        return
    if package.get("source") is not None:
        return
    # A vendored upstream crate is patched in by path, so cargo reports no source
    # and the checks above cannot tell it apart from our own code. Vendoring a
    # dependency does not make us its author, and the forbid(unsafe_code) rule
    # below is about code we write. Its unsafe is still counted into the snapshot.
    if VENDOR_DIR_NAME in relative_manifest.parts:
        return
    name = package["name"]
    targets = [target for target in package["targets"] if target["kind"][0] in {"lib", "bin", "cdylib", "staticlib"}]
    if name in UNSAFE_EXCEPTIONS:
        if not lint_policy_declared(manifest_path, "rust", "unsafe_op_in_unsafe_fn"):
            raise RuntimeError(f"unsafe exception {name} does not deny unsafe_op_in_unsafe_fn")
        if name not in UNDOCUMENTED_UNSAFE_POLICY_EXCEPTIONS and not lint_policy_declared(
            manifest_path, "clippy", "undocumented_unsafe_blocks"
        ):
            raise RuntimeError(f"unsafe exception {name} does not deny undocumented unsafe blocks")
        return
    for target in targets:
        source = Path(target["src_path"]).read_text(encoding="utf-8")
        if "#![forbid(unsafe_code)]" not in source:
            relative = Path(target["src_path"]).relative_to(ROOT)
            raise RuntimeError(f"shipped first-party target lacks forbid(unsafe_code): {relative}")


def lint_policy_declared(manifest_path: Path, group: str, lint: str) -> bool:
    """Accept an explicit lint or a workspace lint the package actually inherits."""
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    lints = manifest.get("lints", {})
    if lints.get(group, {}).get(lint) == "deny":
        return True
    if lints.get("workspace") is not True:
        return False

    for directory in manifest_path.parents:
        candidate = directory / "Cargo.toml"
        if not candidate.is_file():
            continue
        workspace = tomllib.loads(candidate.read_text(encoding="utf-8")).get("workspace")
        if workspace is not None:
            return workspace.get("lints", {}).get(group, {}).get(lint) == "deny"
    return False


def build_snapshot() -> dict:
    packages: dict[str, dict] = {}
    for graph, manifest, target in GRAPHS:
        metadata = run_metadata(manifest, target)
        by_id = {package["id"]: package for package in metadata["packages"]}
        nodes = metadata.get("resolve", {}).get("nodes", [])
        node_by_id = {node["id"]: node for node in nodes}
        root_id = metadata.get("resolve", {}).get("root")
        pending = [root_id] if root_id else list(metadata.get("workspace_default_members", []))
        reachable: set[str] = set()
        while pending:
            package_id = pending.pop()
            if not package_id or package_id in reachable:
                continue
            reachable.add(package_id)
            pending.extend(node_by_id.get(package_id, {}).get("dependencies", []))
        for node in nodes:
            if node["id"] not in reachable:
                continue
            package = by_id[node["id"]]
            manifest_path = Path(package["manifest_path"]).resolve()
            assert_first_party_policy(package, manifest_path)
            key = f'{package["name"]}@{package["version"]}|{package.get("source") or manifest_path.parent}'
            record = packages.get(key)
            if record is None:
                record = {
                    "name": package["name"],
                    "version": package["version"],
                    "source": display_source(package, manifest_path),
                    "graphs": {},
                    "unsafe": unsafe_counts(manifest_path.parent),
                }
                packages[key] = record
            record["graphs"][graph] = sorted(node.get("features", []))
    ordered = sorted(
        packages.values(), key=lambda item: (item["name"], item["version"], item["source"])
    )
    return {
        "schema": 1,
        "generator": "validation/security/unsafe-audit.py",
        "graphs": [
            {"name": name, "manifest": manifest, "target": target}
            for name, manifest, target in GRAPHS
        ],
        "unsafe_exceptions": sorted(UNSAFE_EXCEPTIONS),
        "undocumented_unsafe_policy_exceptions": sorted(UNDOCUMENTED_UNSAFE_POLICY_EXCEPTIONS),
        "packages": ordered,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true", help="replace the reviewed baseline")
    parser.add_argument("--self-test", action="store_true", help="run lexer regression tests")
    arguments = parser.parse_args()
    if arguments.self_test:
        sample = r'''
// unsafe { comment }
/* unsafe fn nested() { /* unsafe impl */ } */
const A: &str = "unsafe { string }";
const B: &str = r###"unsafe fn raw()"###;
const C: char = 'u';
unsafe fn real() {}
unsafe impl Send for X {}
macro_rules! x { () => { unsafe { call() } } }
'''
        tokens = list(rust_tokens(sample))
        assert tokens.count("unsafe") == 3, tokens
        assert [tokens[index + 1] for index, token in enumerate(tokens) if token == "unsafe"] == [
            "fn",
            "impl",
            "{",
        ]
        print("unsafe lexer self-test passed")
        return 0
    try:
        snapshot = build_snapshot()
    except RuntimeError as error:
        print(f"unsafe audit failed: {error}", file=sys.stderr)
        return 2
    rendered = json.dumps(snapshot, indent=2, sort_keys=True) + "\n"
    if arguments.write:
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(rendered, encoding="utf-8")
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 0
    if not BASELINE.exists():
        print("unsafe baseline missing; run validation/security/unsafe-audit.py --write", file=sys.stderr)
        return 1
    expected = BASELINE.read_text(encoding="utf-8")
    if expected != rendered:
        print("unsafe dependency snapshot drifted; review and run validation/security/unsafe-audit.py --write", file=sys.stderr)
        return 1
    print("unsafe dependency snapshot matches reviewed baseline")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
