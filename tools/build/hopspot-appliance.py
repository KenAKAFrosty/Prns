#!/usr/bin/env python3
"""Build the OpenWrt application slot manager; never a firmware image."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("g4_build", Path(__file__).with_name("hopspot-g4.py"))
G4 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(G4)
CRATE = ROOT / "personal-hopspot/appliance"
BINARY = "personal-hopspot-appliance"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New manager bundle directory")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/hopspot-appliance/cargo")
    parser.add_argument("--qualification-output", type=Path, help="Separate, explicitly lab-only executable output")
    args = parser.parse_args()
    output = args.output.resolve()
    qualification = args.qualification_output.resolve() if args.qualification_output else None
    if output.exists() or (qualification and (qualification.exists() or qualification.is_relative_to(output))):
        parser.error("outputs must be new; qualification output must stay outside the shipping bundle")
    zig = args.zig.resolve(strict=True)
    if G4.capture([str(zig), "version"]) != G4.ZIG_VERSION:
        parser.error("Zig differs from the qualified version")
    rust = G4.capture(["rustc", f"+{G4.RUST_TOOLCHAIN}", "-vV"])
    if f"commit-hash: {G4.RUST_COMMIT}" not in rust.splitlines():
        parser.error("compiler differs from the qualified commit")
    linker = str(ROOT / "personal-hopspot/headless/scripts/mipsel-musl-cc")
    env = dict(os.environ)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env.pop("RUSTFLAGS", None)
    env.update({"ZIG": str(zig), "CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER": linker,
                "CC_mipsel_unknown_linux_musl": linker,
                "CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_RUSTFLAGS": G4.RUSTFLAGS,
                "CARGO_PROFILE_RELEASE_OPT_LEVEL": "s", "CARGO_PROFILE_RELEASE_STRIP": "symbols"})
    command = ["cargo", f"+{G4.RUST_TOOLCHAIN}", "build", "--locked", "--release",
               "--package", BINARY, "--target", G4.TARGET, "--target-dir", str(args.target_dir),
               "-Z", "build-std=std,panic_abort", "--bin", BINARY]
    if qualification:
        command.extend(["--example", "qualification"])
    subprocess.run(command, cwd=ROOT, env=env, check=True)
    executable = args.target_dir / G4.TARGET / "release" / BINARY
    G4.verify_elf(executable.read_bytes())
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hopspot-appliance-", dir=output.parent) as temporary:
        staging = Path(temporary) / "bundle"
        staging.mkdir()
        shutil.copy2(executable, staging / "manager")
        shutil.copytree(CRATE / "openwrt", staging / "openwrt")
        shutil.copytree(CRATE / "docs", staging / "docs")
        shutil.copy2(CRATE / "README.md", staging / "INSTALL.md")
        metadata = {"schema": 1, "artifact_kind": "linux-application-slot-manager-development-bundle",
                    "target": G4.TARGET, "source_commit": G4.capture(["git", "rev-parse", "HEAD"]),
                    "working_tree_dirty": bool(G4.capture(["git", "status", "--porcelain"])),
                    "rustc": rust, "zig_version": G4.ZIG_VERSION,
                    "binary": {"bytes": executable.stat().st_size, "sha256": G4.sha256(executable)},
                    "trust": "repository-pinned release Minisign public key; no CLI override",
                    "cargo_lock_sha256": G4.sha256(ROOT / "Cargo.lock")}
        (staging / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
        (staging / "SHA256SUMS").write_text("".join(
            f"{G4.sha256(path)}  {path.relative_to(staging).as_posix()}\n"
            for path in sorted(staging.rglob("*")) if path.is_file()))
        staging.rename(output)
    if qualification:
        lab = args.target_dir / G4.TARGET / "release/examples/qualification"
        G4.verify_elf(lab.read_bytes())
        qualification.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(lab, qualification)
        print(f"Lab-only manager (not bundled): {qualification}; sha256={G4.sha256(qualification)}")
    print(f"Application manager development bundle: {output}")


if __name__ == "__main__":
    main()
