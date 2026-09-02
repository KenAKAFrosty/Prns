#!/usr/bin/env python3
"""Run the expensive CI checks that are justified by the pushed diff.

The ordinary pre-push hook already verifies formatting, generated contracts, and
every host-compatible Cargo workspace. This companion gate covers the important
compile modes that a plain ``cargo check`` cannot see, while keeping unrelated
pushes fast.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence


ZERO_SHA = "0" * 40
ROOT = Path(__file__).resolve().parents[2]


@dataclass(frozen=True)
class Gate:
    name: str
    command: tuple[str, ...]
    cwd: Path = ROOT
    env: tuple[tuple[str, str], ...] = ()


def _has_prefix(paths: set[str], prefixes: Iterable[str]) -> bool:
    return any(path.startswith(prefix) for path in paths for prefix in prefixes)


def plan_for_paths(paths: set[str]) -> tuple[Gate, ...]:
    gates: list[Gate] = []

    root_rust = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "prns-core/",
                "prns-macros/",
                "prns-host/core/",
                "prns-runtime/core/",
                "personal-hopspot/core/",
                "personal-hopspot/sdk/hopspot/",
                "prns-flash-manifest/",
                "prns-nrf-dfu/",
                "prns-nrf-dfu-wasm/",
            ),
        )
    )
    if root_rust:
        gates.append(
            Gate(
                "root Clippy",
                (
                    "cargo",
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
            )
        )

    if _has_prefix(paths, ("prns-core/",)):
        gates.append(
            Gate(
                "prns-core external-allocation lane",
                ("bash", "validation/native/external-alloc.sh"),
            )
        )

    embedded_surface = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "personal-hopspot/core/",
                "personal-hopspot/embedded/esp32/",
                "personal-hopspot/embedded/nrf52840/",
                "prns-core/",
                "prns-interfaces/impls/embassy/",
                "prns-runtime/core/",
                "prns-runtime/impls/embassy/",
                "validation/platforms/embedded.sh",
                "validation/platforms/no-std-esp-build.sh",
            ),
        )
    )
    if embedded_surface:
        gates.append(
            Gate(
                "embedded build matrix",
                ("bash", "validation/platforms/embedded.sh"),
            )
        )
        gates.append(
            Gate(
                "Embassy runtime Clippy",
                (
                    "cargo",
                    "clippy",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                ROOT / "prns-runtime/impls/embassy",
                (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )

    shared_native = _has_prefix(
        paths,
        (
            "personal-rns/",
            "prns-core/",
            "prns-host/",
            "prns-interfaces/impls/tokio/",
            "prns-runtime/impls/tokio/",
        ),
    )
    if shared_native or _has_prefix(paths, ("prns-napi/",)):
        gates.append(
            Gate(
                "Node native binding Clippy",
                ("cargo", "clippy", "--locked", "--", "-D", "warnings"),
                ROOT / "prns-napi",
            )
        )

    tokio_runtime_surface = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "prns-core/",
                "prns-interfaces/impls/tokio/",
                "prns-runtime/core/",
                "prns-runtime/impls/tokio/",
            ),
        )
    )
    if tokio_runtime_surface:
        gates.append(
            Gate(
                "Tokio runtime all-features Clippy",
                (
                    "cargo",
                    "clippy",
                    "--all-features",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                ROOT / "prns-runtime/impls/tokio",
                (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )
        gates.append(
            Gate(
                "Tokio umbrella feature-family Clippy",
                (
                    "cargo",
                    "clippy",
                    "-p",
                    "personal-rns",
                    "--features",
                    "tokio-host,tcp,udp,wifi-auto,shared-instance",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                env=(("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )

    integration_surface = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "prns-core/",
                "prns-interfaces/impls/tokio/",
                "prns-runtime/core/",
                "prns-runtime/impls/tokio/",
                "validation/integration/",
            ),
        )
    )
    if integration_surface:
        gates.append(
            Gate(
                "validation integration capstones Clippy",
                (
                    "cargo",
                    "clippy",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                ROOT / "validation/integration",
                (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )

    daemon_surface = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "prns-core/",
                "prns-config/",
                "prns-interfaces/impls/tokio/",
                "prns-runtime/core/",
                "prns-runtime/impls/tokio/",
                "prnsd/",
            ),
        )
    )
    if daemon_surface:
        gates.append(
            Gate(
                "prnsd all-features Clippy",
                (
                    "cargo",
                    "clippy",
                    "--workspace",
                    "--all-features",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                ROOT / "prnsd",
                (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )

    wasm_surface = (
        "Cargo.toml" in paths
        or "Cargo.lock" in paths
        or _has_prefix(
            paths,
            (
                "personal-rns/",
                "prns-core/",
                "prns-host/core/",
                "prns-host/impls/cooperative/",
                "prns-wasm/",
            ),
        )
    )
    if wasm_surface:
        gates.append(
            Gate(
                "prns-wasm wasm32 Clippy",
                (
                    "cargo",
                    "clippy",
                    "--target",
                    "wasm32-unknown-unknown",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ),
                ROOT / "prns-wasm",
                (("RUSTFLAGS", "-D warnings --cfg aes_armv8"),),
            )
        )

    host_contract = _has_prefix(
        paths,
        (
            "prns-host/schema/",
            "prns-host/core/",
            "tools/repo/generate-host-contract.py",
        ),
    )
    if host_contract or _has_prefix(paths, ("prns-js/",)):
        gates.extend(
            (
                Gate(
                    "JavaScript clean generated output",
                    ("npm", "run", "clean"),
                    ROOT / "prns-js",
                ),
                Gate(
                    "JavaScript and TypeScript contract check",
                    ("npm", "run", "check"),
                    ROOT / "prns-js",
                ),
            )
        )

    if host_contract or _has_prefix(paths, ("prns-host/bindings/jvm/",)):
        wrapper = "gradlew.bat" if os.name == "nt" else "./gradlew"
        gates.append(
            Gate(
                "JVM binding compile",
                (wrapper, "classes", "testClasses", "--no-daemon"),
                ROOT / "prns-host/bindings/jvm",
            )
        )

    if host_contract or _has_prefix(paths, ("prns-host/bindings/swift/",)):
        gates.append(
            Gate(
                "Swift host contract smoke",
                (
                    "python3",
                    "-m",
                    "validation.interop.cases.host_swift_contract_smoke",
                ),
            )
        )

    lock_paths = sorted(
        path
        for path in paths
        if path.endswith("Cargo.lock") and "/vendor/" not in path
    )
    dependency_policy_surface = bool(lock_paths) or bool(
        paths
        & {
            "about.toml",
            "deny.toml",
            "validation/security/deps-audit.sh",
            "validation/security/license-policy-parity.py",
            "validation/security/npm-production-audit.py",
        }
    )
    if dependency_policy_surface:
        gates.append(
            Gate(
                "license policy parity",
                ("python3", "validation/security/license-policy-parity.py"),
            )
        )

    for lock_path in lock_paths:
        manifest = ROOT / Path(lock_path).parent / "Cargo.toml"
        if not manifest.is_file():
            continue
        gates.append(
            Gate(
                f"dependency policy ({lock_path})",
                (
                    "cargo",
                    "deny",
                    "--manifest-path",
                    str(manifest),
                    "--locked",
                    "--exclude-dev",
                    "check",
                    "--config",
                    str(ROOT / "deny.toml"),
                    "advisories",
                    "licenses",
                    "sources",
                    "bans",
                ),
            )
        )

    unsafe_surface = bool(lock_paths) or any(
        path.endswith(".rs") or path.endswith("Cargo.toml") for path in paths
    ) or bool(
        paths
        & {
            "audits/unsafe-snapshot.json",
            "validation/security/unsafe-audit.py",
        }
    )
    if unsafe_surface:
        gates.append(
            Gate(
                "unsafe dependency inventory",
                ("python3", "validation/security/unsafe-audit.py"),
            )
        )

    return tuple(gates)


def changed_paths(updates: Sequence[tuple[str, str]]) -> set[str]:
    paths: set[str] = set()
    for local_sha, remote_sha in updates:
        if remote_sha == ZERO_SHA:
            command = ("git", "ls-tree", "-r", "--name-only", local_sha)
        else:
            command = ("git", "diff", "--name-only", remote_sha, local_sha)
        result = subprocess.run(
            command,
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        paths.update(line for line in result.stdout.splitlines() if line)
    return paths


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--update",
        action="append",
        nargs=2,
        metavar=("LOCAL_SHA", "REMOTE_SHA"),
        required=True,
        help="one ref update read by the pre-push hook",
    )
    parser.add_argument(
        "--plan",
        action="store_true",
        help="print selected checks without running them",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    paths = changed_paths(tuple(map(tuple, args.update)))
    gates = plan_for_paths(paths)

    if not gates:
        print("[pre-push-ci-parity] no additional CI lanes selected")
        return 0

    print("[pre-push-ci-parity] selected:")
    for gate in gates:
        print(f"  - {gate.name}")
    if args.plan:
        return 0

    for gate in gates:
        print(f"\n[pre-push-ci-parity] {gate.name}", flush=True)
        result = subprocess.run(
            gate.command,
            cwd=gate.cwd,
            env={**os.environ, **dict(gate.env)},
            check=False,
        )
        if result.returncode != 0:
            print(
                f"\npre-push CI parity failed: {gate.name}",
                file=sys.stderr,
            )
            return result.returncode

    print("\nPRE_PUSH_CI_PARITY_COMPLETE")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
