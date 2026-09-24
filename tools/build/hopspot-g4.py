#!/usr/bin/env python3
"""Build a development application bundle for the ThinkNode G4 vendor OS."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "personal-hopspot/headless"
TARGET = "mipsel-unknown-linux-musl"
BINARY = "personal-hopspot-headless"
RUST_COMMIT = "6bdf43094fae65d298bc430362f116176cd25a3c"
ZIG_VERSION = "0.15.2"
RUST_TOOLCHAIN = "nightly-2026-06-02"
RUSTFLAGS = "-C link-self-contained=no -C target-feature=+crt-static"


def capture(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_elf(data):
    """Reject another host/ABI or a binary needing the device's dynamic loader."""
    if len(data) < 52 or data[:7] != b"\x7fELF\x01\x01\x01":
        raise ValueError("expected an ELF32 little-endian executable")
    header = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
    kind, machine, version, _, phoff, _, flags, ehsize, phsize, phnum, *_ = header
    if (kind, machine, version, ehsize) != (2, 8, 1, 52):
        raise ValueError("expected a MIPS executable")
    if flags & 0xF000F000 != 0x70001000:
        raise ValueError("expected the MIPS32r2 O32 ABI")
    if phsize != 32 or not phnum or phoff < 52 or phoff + phsize * phnum > len(data):
        raise ValueError("invalid ELF program-header table")
    segments = [struct.unpack_from("<I", data, phoff + phsize * i)[0] for i in range(phnum)]
    if 1 not in segments or any(segment in (2, 3) for segment in segments):
        raise ValueError("expected static load segments without PT_DYNAMIC or PT_INTERP")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", type=Path, required=True, help="Verified Zig 0.15.2 executable")
    parser.add_argument("--toolchain", default=RUST_TOOLCHAIN, help="Rustup toolchain; exact compiler commit is checked")
    parser.add_argument("--output", type=Path, required=True, help="New application bundle directory; must not exist")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/hopspot-g4/cargo")
    parser.add_argument("--with-probe", action="store_true", help="Include the bounded page-fetch qualification executable")
    parser.add_argument("--with-websocket", action="store_true", help="Include the optional plain WebSocket server")
    parser.add_argument("--with-auto-wifi", action="store_true", help="Include explicitly scoped Auto-WiFi and native DNS-SD")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("output already exists; choose a new candidate directory")
    zig = args.zig.resolve(strict=True)
    rust = capture(["rustc", f"+{args.toolchain}", "-vV"])
    if f"commit-hash: {RUST_COMMIT}" not in rust.splitlines():
        parser.error("compiler differs from the qualified 2026-06-01 nightly")
    if capture([str(zig), "version"]) != ZIG_VERSION:
        parser.error(f"Zig {ZIG_VERSION} is required")
    target_dir = args.target_dir.resolve()
    linker = str(CRATE / "scripts/mipsel-musl-cc")
    env = dict(os.environ)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env.pop("RUSTFLAGS", None)
    env.update({
        "ZIG": str(zig),
        "CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER": linker,
        "CC_mipsel_unknown_linux_musl": linker,
        "CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_RUSTFLAGS": RUSTFLAGS,
    })
    features = ["wifi-halow"] + (["websocket"] if args.with_websocket else [])
    if args.with_auto_wifi:
        features.append("wifi-auto")
    command = [
        "cargo", f"+{args.toolchain}", "build", "--locked", "--release",
        "--manifest-path", str(CRATE / "Cargo.toml"), "--target", TARGET,
        "--features", ",".join(features), "--target-dir", str(target_dir), "-Z", "build-std=std,panic_abort", "--bin", BINARY,
    ]
    if args.with_probe:
        command.extend(["--example", "fetch_page"])
    subprocess.run(command, cwd=ROOT, env=env, check=True)
    executable = target_dir / TARGET / "release" / BINARY
    verify_elf(executable.read_bytes())
    metadata = {
        "schema": 1,
        "artifact_kind": "linux-application-development-bundle",
        "qualification": "TCP plus experimental HaLoW; not a firmware image or a signed public release",
        "cargo_features": features,
        "target": TARGET,
        "source_commit": capture(["git", "rev-parse", "HEAD"]),
        "working_tree_dirty": bool(capture(["git", "status", "--porcelain"])),
        "rustc": rust,
        "cargo": capture(["cargo", f"+{args.toolchain}", "-V"]),
        "zig_version": ZIG_VERSION,
        "zig_executable_sha256": sha256(zig),
        "cargo_lock_sha256": sha256(CRATE / "Cargo.lock"),
        "rustflags": RUSTFLAGS,
        "c_target": "mipsel-linux-musleabi -mcpu=mips32r2 -msoft-float",
        "binary": {"path": BINARY, "bytes": executable.stat().st_size, "sha256": sha256(executable)},
    }
    probe = target_dir / TARGET / "release/examples/fetch_page"
    if args.with_probe:
        verify_elf(probe.read_bytes())
        metadata["probe"] = {"path": "fetch_page", "bytes": probe.stat().st_size, "sha256": sha256(probe)}
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hopspot-g4-", dir=output.parent) as temporary:
        staging = Path(temporary) / "bundle"
        staging.mkdir()
        shutil.copy2(executable, staging / BINARY)
        if args.with_probe:
            shutil.copy2(probe, staging / "fetch_page")
        shutil.copy2(CRATE / "docs/thinknode-g4.md", staging / "INSTALL.md")
        shutil.copy2(CRATE / "docs/networking.md", staging / "networking.md")
        (staging / "qualification").mkdir()
        for report in (
            "auto-wifi-g4-2026-09-29.md",
            "auto-wifi-dnssd-g4-2026-09-29.md",
            "auto-wifi-dnssd-g4-2026-09-29.json",
        ):
            shutil.copy2(CRATE / "docs/qualification" / report, staging / "qualification" / report)
        shutil.copy2(ROOT / "LICENSE-MIT", staging / "LICENSE-MIT")
        shutil.copy2(ROOT / "LICENSE-APACHE", staging / "LICENSE-APACHE")
        shutil.copy2(ROOT / "THIRD_PARTY_NOTICES.md", staging / "THIRD_PARTY_NOTICES.md")
        (staging / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
        files = sorted(path for path in staging.rglob("*") if path.is_file())
        (staging / "SHA256SUMS").write_text("".join(f"{sha256(path)}  {path.relative_to(staging).as_posix()}\n" for path in files))
        staging.rename(output)
    print(f"G4 development application bundle: {output}")


if __name__ == "__main__":
    main()
