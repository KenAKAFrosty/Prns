from __future__ import annotations

import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "tools" / "release"
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from script_command import script_command

from flasher_build_metadata import EXPECTED_TOOLS, EXPECTED_WEB_PACKAGES
from flasher_reproducibility import SEPARATE_ENVELOPES, payload_identity, payload_manifest
from flasher_sparse_sizes import build_report as build_sparse_size_report
from flasher_manifest import (
    validate_nrf_serial_dfu_recovery_artifact,
    validate_uf2_artifact,
)
from source_snapshot import package_source_snapshot


VERSION = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
SOURCE_COMMIT = subprocess.run(
    ("git", "rev-parse", "HEAD"),
    cwd=ROOT,
    text=True,
    capture_output=True,
    check=True,
).stdout.strip()
SOURCE_DATE_EPOCH = 1_774_358_400
ACCEPTANCE_COMMIT = "b" * 40
KEY_ID = "0123456789ABCDEF"
REPOSITORY = "example/Prns"
CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def uf2_payload(application_base: int) -> bytes:
    block = bytearray(512)
    for offset, value in (
        (0, 0x0A324655),
        (4, 0x9E5D5157),
        (8, 0x00002000),
        (12, application_base),
        (16, 256),
        (20, 0),
        (24, 1),
        (28, 0xADA52840),
        (508, 0x0AB16F30),
    ):
        block[offset : offset + 4] = value.to_bytes(4, "little")
    block[32:288] = bytes(range(256))
    return bytes(block)


def run_script(script: str, *arguments: object, environment: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    target = SCRIPTS / script
    command = script_command(target, *arguments)
    return subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )


def run_task(*arguments: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(ROOT / "tools" / "prns"), *(str(argument) for argument in arguments)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def fake_signer(path: Path) -> None:
    path.write_text(
        """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-S" ]]; then
  document=""
  signature=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -m) document="$2"; shift 2 ;;
      -x) signature="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  test -f "$document"
  printf 'fixture-signature:%s\n' "$(sha256sum "$document" | awk '{print $1}')" > "$signature"
  exit 0
fi
exit 0
""",
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class CandidateFixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        root.mkdir(parents=True)
        self.key = root.parent / "minisign.pub"
        self.repository_version = root.parent / "VERSION"
        self.key.write_text(
            f"untrusted comment: minisign public key {KEY_ID}\nRWQfixturepublickey\n",
            encoding="utf-8",
        )
        self.repository_version.write_text(f"{VERSION}\n", encoding="utf-8")
        (root / "minisign.pub").write_bytes(self.key.read_bytes())
        (root / "VERSION").write_text(f"{VERSION}\n", encoding="utf-8")
        (root / "website").mkdir(parents=True)
        (root / "website" / "index.html").write_text("fixture site\n", encoding="utf-8")
        (root / "website" / "404.html").write_text("fixture site\n", encoding="utf-8")
        flasher_bundle = root / "website" / "assets" / "flasher" / "prns-flash.js"
        flasher_bundle.parent.mkdir(parents=True)
        flasher_bundle.write_text("export const fixture = true;\n", encoding="utf-8")
        nrf_dfu_assets = flasher_bundle.parent / "nrf-dfu"
        nrf_dfu_assets.mkdir()
        for name in (
            "prns_nrf_dfu_core.js",
            "prns_nrf_dfu_core.d.ts",
            "prns_nrf_dfu_core_bg.wasm",
            "prns_nrf_dfu_core_bg.wasm.d.ts",
        ):
            (nrf_dfu_assets / name).write_bytes(f"fixture {name}\n".encode())
        browser_wasm = (
            root
            / "website"
            / "browser-node-playground-console"
            / "pkg"
            / "prns_wasm_bg.wasm"
        )
        browser_wasm.parent.mkdir(parents=True)
        browser_wasm.write_bytes(b"fixture source-enabled wasm")
        package_source_snapshot(
            repository=ROOT,
            commit=SOURCE_COMMIT,
            version=VERSION,
            output=root / "website" / "source.zip",
            metadata=root / "metadata" / "source.json",
        )
        source_metadata = json.loads(
            (root / "metadata" / "source.json").read_text(encoding="utf-8")
        )
        source_archive = (root / "website" / "source.zip").read_bytes()
        browser_wasm.write_bytes(
            b"fixture source-enabled wasm "
            + source_archive
            + b" "
            + source_metadata["sha256"].encode()
            + b" "
            + SOURCE_COMMIT[:12].encode()
            + b" /file/source.zip /file/source.zip.sha256"
        )
        (root / "LICENSE-APACHE").write_text("fixture Apache license\n", encoding="utf-8")
        (root / "LICENSE-MIT").write_text("fixture MIT license\n", encoding="utf-8")
        (root / "THIRD_PARTY_NOTICES.md").write_text("fixture notices\n", encoding="utf-8")
        targets = []
        self.firmware_paths = []
        for index, board in enumerate(
            (
                "heltec-v4",
                "heltec-v4-r8",
                "t-beam-supreme",
                "xiao-esp32-c6",
                "t-echo",
                "t114",
                "t096",
                "t1000-e",
            ),
            start=1,
        ):
            filenames = (
                ("t-echo-s140-6.1.1.uf2", "t-echo-s140-7.3.0.uf2")
                if board == "t-echo"
                else ("heltec-t114-s140-6.1.1.uf2",)
                if board == "t114"
                else ("t096-s140-6.1.1.uf2",)
                if board == "t096"
                else ("t1000e.bin", "t1000e.dat", "t1000e.uf2")
                if board == "t1000-e"
                else ("application.bin",)
            )
            artifacts = []
            for filename in filenames:
                relative = f"firmware/{board}/{filename}"
                artifact = root / relative
                artifact.parent.mkdir(parents=True, exist_ok=True)
                if board in {"t-echo", "t114", "t096"}:
                    application_base = 0x26000 if "6.1.1" in filename else 0x27000
                    artifact.write_bytes(uf2_payload(application_base))
                elif board == "t1000-e" and filename == "t1000e.bin":
                    artifact.write_bytes(bytes(range(256)))
                elif board == "t1000-e" and filename == "t1000e.uf2":
                    artifact.write_bytes(uf2_payload(0x27000))
                else:
                    artifact.write_bytes(f"firmware-{index}-{board}-{filename}".encode())
                self.firmware_paths.append(artifact)
                hosted = root / "website" / "releases" / VERSION / relative
                hosted.parent.mkdir(parents=True, exist_ok=True)
                hosted.write_bytes(artifact.read_bytes())
                artifacts.append(
                    {
                        "path": relative,
                        "size": artifact.stat().st_size,
                        "sha256": sha256(artifact),
                    }
                )
            if board == "t-echo":
                parts = []
                variants = [
                    {
                        **artifact,
                        "softdevice_family": "s140",
                        "softdevice_version": version,
                        "fwid": fwid,
                        "application_base": application_base,
                        "family_id": "0xada52840",
                    }
                    for artifact, version, fwid, application_base in zip(
                        artifacts,
                        ("6.1.1", "7.3.0"),
                        ("0x00b6", "0x0123"),
                        ("0x00026000", "0x00027000"),
                    )
                ]
            elif board in {"t114", "t096"}:
                parts = []
                variants = [
                    {
                        **artifacts[0],
                        "softdevice_family": "s140",
                        "softdevice_version": "6.1.1",
                        "fwid": "0x00b6",
                        "application_base": "0x00026000",
                        "family_id": "0xada52840",
                    }
                ]
            elif board == "t1000-e":
                parts = []
                variants = []
            else:
                parts = [{**artifacts[0], "kind": "application", "offset": 0x10000}]
                variants = []
            target = {
                "board_slug": board,
                "transport": (
                    "uf2-mass-storage"
                    if board in {"t-echo", "t114", "t096"}
                    else "nrf-serial-dfu"
                    if board == "t1000-e"
                    else "esp-serial"
                ),
                "parts": parts,
                "variants": variants,
            }
            if board == "t1000-e":
                target["nrf_serial_dfu"] = {
                    "application": {**artifacts[0], "kind": "dfu-application"},
                    "init_packet": {**artifacts[1], "kind": "dfu-init-packet"},
                    "compatibility": {
                        "softdevice_family": "s140",
                        "softdevice_version": "7.3.0",
                        "fwid": "0x0123",
                        "application_base": "0x00027000",
                        "application_end_exclusive": "0x000ea000",
                    },
                    "recovery": {
                        "mount_label": "T1000-E",
                        "board_id_prefix": "nrf52840-t1000-e-v1",
                        "family_id": "0xada52840",
                        "artifact": {**artifacts[2], "kind": "uf2"},
                    },
                }
            targets.append(target)
        write_json(
            root / "metadata" / "source-capabilities.json",
            {
                "schema": 1,
                "version": VERSION,
                "commit": SOURCE_COMMIT,
                "targets": [
                    {
                        "schema": 1,
                        "board_slug": board,
                        "nominally_capable": False,
                        "status": "absent",
                        "source": None,
                        "reserve_bytes": None,
                    }
                    for board in (
                        "heltec-v4",
                        "heltec-v4-r8",
                        "t-beam-supreme",
                        "xiao-esp32-c6",
                        "t-echo",
                        "t114",
                        "t096",
                        "t1000-e",
                    )
                ],
            },
        )
        self.manifest = {
            "schema": 3,
            "release": {
                "version": VERSION,
                "channel": "stable",
                "commit": SOURCE_COMMIT,
            },
            "signing": {"key_id": KEY_ID},
            "targets": targets,
        }
        self.manifest_path = root / "flash-manifest.json"
        write_json(self.manifest_path, self.manifest)
        hosted_manifest = root / "website" / "releases" / VERSION / "flash-manifest.json"
        hosted_manifest.write_bytes(self.manifest_path.read_bytes())
        self.channel = {
            "schema": 1,
            "channel": "stable",
            "version": VERSION,
            "manifest_url": f"https://reticulum.rs/releases/{VERSION}/flash-manifest.json",
            "manifest_sha256": sha256(self.manifest_path),
        }
        self.channel_path = root / "channels" / "stable.json"
        write_json(self.channel_path, self.channel)
        hosted_channel = root / "website" / "releases" / "channels" / "stable.json"
        hosted_channel.parent.mkdir(parents=True, exist_ok=True)
        hosted_channel.write_bytes(self.channel_path.read_bytes())
        write_json(
            root / "metadata" / "build.json",
            {
                "schema": 2,
                "source_commit": SOURCE_COMMIT,
                "source_date_epoch": SOURCE_DATE_EPOCH,
                "built_at_utc": datetime.fromtimestamp(
                    SOURCE_DATE_EPOCH, timezone.utc
                ).replace(microsecond=0).isoformat(),
                "timestamp_source": "source_commit",
                "host": {"system": "Linux", "machine": "x86_64"},
                "expected_tools": EXPECTED_TOOLS,
                "tools": {
                    "rustc": "rustc 1.96.0 (fixture)",
                    "cargo": "cargo 1.96.0 (fixture)",
                    "node": "v24.18.0",
                    "npm": "11.0.0",
                    "dioxus": "dioxus 0.7.5",
                    "wasm_bindgen": "wasm-bindgen 0.2.126",
                    "cargo_binstall": "cargo-binstall 1.21.0",
                    "espup": "espup 0.17.1",
                    "esp_rustc": "rustc 1.95.0-nightly (fixture)",
                    "xtensa_gcc": "xtensa-esp-elf-gcc (crosstool-NG esp-15.2.0_20250920) 15.2.0",
                    "llvm_objcopy": "llvm-objcopy version 20.1.8",
                    "python": "Python 3.13.0",
                    "git": "git version 2.50.0",
                },
                "web_packages": EXPECTED_WEB_PACKAGES,
            },
        )
        write_json(
            root / "metadata" / "release-history.json",
            {
                "schema": 1,
                "mode": "bootstrap",
                "head": None,
                "tree": {
                    "file_count": 0,
                    "total_bytes": 0,
                    "tree_sha256": hashlib.sha256(b"").hexdigest(),
                },
                "files": [],
            },
        )
        write_json(
            root / "metadata" / "sparse-sizes.json",
            build_sparse_size_report(self.manifest),
        )
        audit = root / "audit" / "release-audit-evidence.md"
        audit.parent.mkdir(parents=True, exist_ok=True)
        audit.write_text("fixture release audit\n", encoding="utf-8")
        qualification = root / "qualification"
        qualification.mkdir()
        qualification_sources = {
            "QUALIFICATION.md": ROOT / "release" / "acceptance" / "QUALIFICATION.md",
            "create-flasher-acceptance.py": SCRIPTS / "create-flasher-acceptance.py",
            "validate-flasher-acceptance.py": SCRIPTS / "validate-flasher-acceptance.py",
            "flasher_acceptance_contract.py": SCRIPTS / "flasher_acceptance_contract.py",
            "flasher_manifest.py": SCRIPTS / "flasher_manifest.py",
            "serve-flasher-candidate.py": SCRIPTS / "serve-flasher-candidate.py",
            "verify-flasher-candidate-files.py": SCRIPTS / "verify-flasher-candidate-files.py",
            "validate-flasher-tester-roster.py": SCRIPTS / "validate-flasher-tester-roster.py",
            "flasher_tester_roster.py": SCRIPTS / "flasher_tester_roster.py",
            "flasher_hotfix.py": SCRIPTS / "flasher_hotfix.py",
            "package-flasher-qualification-evidence.py": SCRIPTS
            / "package-flasher-qualification-evidence.py",
        }
        for name, source in qualification_sources.items():
            shutil.copy2(source, qualification / name)
        physical_hosts = {
            ("heltec-v4", "cli"): ("linux", "x86_64"),
            ("heltec-v4", "web"): ("linux", "x86_64"),
            ("heltec-v4-r8", "cli"): ("linux", "x86_64"),
            ("heltec-v4-r8", "web"): ("linux", "x86_64"),
            ("t-beam-supreme", "cli"): ("macos", "aarch64"),
            ("t-beam-supreme", "web"): ("macos", "aarch64"),
            ("xiao-esp32-c6", "cli"): ("windows", "x86_64"),
            ("xiao-esp32-c6", "web"): ("windows", "x86_64"),
            ("t-echo", "cli"): ("linux", "aarch64"),
            ("t-echo", "web"): ("macos", "x86_64"),
            ("t114", "cli"): ("linux", "x86_64"),
            ("t114", "web"): ("macos", "aarch64"),
            ("t096", "cli"): ("linux", "aarch64"),
            ("t096", "web"): ("macos", "aarch64"),
            ("t1000-e", "cli"): ("macos", "x86_64"),
            ("t1000-e", "web"): ("windows", "x86_64"),
        }
        physical_assignments = []
        for (board, surface), (os_name, architecture) in physical_hosts.items():
            assignment = {
                "board": board,
                "surface": surface,
                "os": os_name,
                "architecture": architecture,
                "tester": "github:fixture",
                "cables_ready": True,
                "device_permissions_ready": True,
                "recovery_instructions_reviewed": True,
            }
            if surface == "web":
                assignment["browser"] = {
                    "name": "edge" if os_name == "windows" else "chrome",
                    "channel": "stable",
                }
            physical_assignments.append(assignment)
        roster = {
            "schema": 3,
            "release": {"version": VERSION},
            "release_owner": "github:fixture-owner",
            "confirmed_on": "2025-01-01",
            "physical_assignments": physical_assignments,
            "web_serial_assignments": [
                {
                    "board": board,
                    "os": os_name,
                    "architecture": architecture,
                    "browser": {"name": "firefox", "channel": "stable"},
                    "tester": "github:fixture",
                    "cables_ready": True,
                    "device_permissions_ready": True,
                    "recovery_instructions_reviewed": True,
                }
                for board, os_name, architecture in (
                    ("heltec-v4", "linux", "x86_64"),
                    ("t-beam-supreme", "macos", "x86_64"),
                    ("xiao-esp32-c6", "windows", "x86_64"),
                )
            ],
            "fallback_assignments": [
                {
                    "browser": {"name": browser, "channel": "stable"},
                    "os": os_name,
                    "architecture": architecture,
                    "tester": "github:fixture",
                    "browser_ready": True,
                }
                for browser, os_name, architecture in (("safari", "macos", "aarch64"),)
            ],
            "installation_assignments": [
                {
                    "target": target,
                    "os": os_name,
                    "architecture": architecture,
                    "tester": "github:fixture",
                    "archive_ready": True,
                }
                for target, (os_name, architecture) in {
                    "aarch64-apple-darwin": ("macos", "aarch64"),
                    "x86_64-apple-darwin": ("macos", "x86_64"),
                    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
                    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
                    "x86_64-pc-windows-msvc": ("windows", "x86_64"),
                }.items()
            ],
        }
        self.tester_roster = root.parent / "tester-roster.json"
        write_json(self.tester_roster, roster)
        write_json(qualification / "tester-roster.json", roster)
        cli = root / "cli"
        cli.mkdir()
        self.cli_archives = []
        for index, (target, extension) in enumerate(CLI_TARGETS.items(), start=1):
            archive = cli / f"hopspot-flash-{VERSION}-{target}{extension}"
            archive.write_bytes(f"cli-{index}-{target}".encode())
            self.cli_archives.append(archive)
        (cli / "install.sh").write_text("#!/bin/sh\n", encoding="utf-8")
        (cli / "install.ps1").write_text("# fixture\n", encoding="utf-8")
        (cli / "README.md").write_text("fixture\n", encoding="utf-8")
        write_json(
            root / "metadata" / "reproducibility.json",
            {
                "schema": 1,
                "release": {"version": VERSION, "source_commit": SOURCE_COMMIT},
                "result": "matched",
                "builds": [
                    {"name": "primary", "archive_sha256": "1" * 64},
                    {"name": "reproduction", "archive_sha256": "1" * 64},
                ],
                "payload": payload_identity(payload_manifest(root, exclude_report=True)),
                "comparison": {
                    "archive_bytes_equal": True,
                    "payload_bytes_equal": True,
                },
                "separate_envelopes": SEPARATE_ENVELOPES,
            },
        )
        self.write_sums()

    def write_sums(self) -> None:
        files = sorted(
            path
            for path in self.root.rglob("*")
            if path.is_file() and path.name != "SHA256SUMS.txt" and not path.name.endswith(".minisig")
        )
        (self.root / "SHA256SUMS.txt").write_text(
            "".join(f"{sha256(path)}  {path.relative_to(self.root).as_posix()}\n" for path in files),
            encoding="utf-8",
        )


class FlasherReleaseCustodyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.workspace = Path(self.temporary.name)
        self.fixture = CandidateFixture(self.workspace / "candidate")
        self.signer = self.workspace / "fake-minisign"
        self.secret = self.workspace / "fixture.key"
        fake_signer(self.signer)
        self.secret.write_text("fixture secret\n", encoding="utf-8")
        self.environment = dict(os.environ)
        self.environment.update(
            {
                "PRNS_MINISIGN_BIN": str(self.signer),
                "PRNS_MINISIGN_PUBLIC_KEY": str(self.fixture.key),
            }
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def validate_unsigned(self) -> subprocess.CompletedProcess[str]:
        return run_script(
            "validate-unsigned-flasher-candidate.py",
            self.fixture.root,
            "--expected-commit",
            SOURCE_COMMIT,
            "--repository-version",
            self.fixture.repository_version,
            "--pinned-key",
            self.fixture.key,
            "--tester-roster",
            self.fixture.tester_roster,
        )

    def sign_candidate(self) -> subprocess.CompletedProcess[str]:
        return run_script(
            "sign-flasher-candidate.sh",
            self.fixture.root,
            self.secret,
            environment=self.environment,
        )

    def test_unsigned_candidate_preflight_executes_every_downstream_stage(self) -> None:
        reproduction = CandidateFixture(self.workspace / "reproduction")

        for fixture in (self.fixture, reproduction):
            (fixture.root / "SHA256SUMS.txt").unlink()
            (fixture.root / "channels" / "stable.json").unlink()
            shutil.rmtree(fixture.root / "website" / "releases" / VERSION)
            (
                fixture.root
                / "website"
                / "releases"
                / "channels"
                / "stable.json"
            ).unlink()
            (fixture.root / "metadata" / "sparse-sizes.json").unlink()
            (fixture.root / "metadata" / "reproducibility.json").unlink()

            finalized = run_task(
                "release",
                "candidate",
                "finalize",
                "--",
                fixture.root,
                "--channel",
                "stable",
                "--commit",
                SOURCE_COMMIT,
                "--key-id",
                KEY_ID,
            )
            self.assertEqual(finalized.returncode, 0, finalized.stderr)
            self.assertEqual(
                json.loads(
                    (
                        fixture.root / "metadata" / "sparse-sizes.json"
                    ).read_text(encoding="utf-8")
                ),
                build_sparse_size_report(fixture.manifest),
            )

        primary_archive = self.workspace / "primary.tar.gz"
        reproduction_archive = self.workspace / "reproduction.tar.gz"
        for fixture, archive in (
            (self.fixture, primary_archive),
            (reproduction, reproduction_archive),
        ):
            packaged = run_task(
                "release",
                "candidate",
                "package",
                "--",
                fixture.root,
                archive,
            )
            self.assertEqual(packaged.returncode, 0, packaged.stderr)

        unsigned_archive = self.workspace / "unsigned.tar.gz"
        report = self.workspace / "reproducibility.json"
        compared = run_task(
            "release",
            "candidate",
            "compare",
            "--",
            "--primary",
            primary_archive,
            "--reproduction",
            reproduction_archive,
            "--output",
            unsigned_archive,
            "--report",
            report,
        )
        self.assertEqual(compared.returncode, 0, compared.stderr)

        verified = self.workspace / "verified"
        extracted = run_task(
            "release",
            "candidate",
            "extract",
            "--",
            unsigned_archive,
            verified,
        )
        self.assertEqual(extracted.returncode, 0, extracted.stderr)
        validated = run_task(
            "release",
            "candidate",
            "validate-unsigned",
            "--",
            verified,
            "--expected-commit",
            SOURCE_COMMIT,
            "--repository-version",
            self.fixture.repository_version,
            "--pinned-key",
            self.fixture.key,
            "--tester-roster",
            self.fixture.tester_roster,
        )
        self.assertEqual(validated.returncode, 0, validated.stderr)

    def test_unsigned_candidate_is_fully_bound_before_signing(self) -> None:
        result = self.validate_unsigned()
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.fixture.cli_archives[0]
        archive.write_bytes(b"tampered")
        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reproducibility payload identity", result.stderr)

        report_path = self.fixture.root / "metadata" / "reproducibility.json"
        report = json.loads(report_path.read_text(encoding="utf-8"))
        report["payload"] = payload_identity(
            payload_manifest(self.fixture.root, exclude_report=True)
        )
        write_json(report_path, report)
        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch", result.stderr)

    def test_unsigned_candidate_requires_the_nordic_dfu_browser_core(self) -> None:
        adapter = (
            self.fixture.root
            / "website"
            / "assets"
            / "flasher"
            / "nrf-dfu"
            / "prns_nrf_dfu_core_bg.wasm"
        )
        adapter.unlink()
        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "candidate required release file is missing or empty: "
            "website/assets/flasher/nrf-dfu/prns_nrf_dfu_core_bg.wasm",
            result.stderr,
        )

    def test_release_boundary_rejects_structurally_invalid_uf2(self) -> None:
        target = next(
            target
            for target in self.fixture.manifest["targets"]
            if target["board_slug"] == "t-echo"
        )
        variant = target["variants"][0]
        payload = bytearray((self.fixture.root / variant["path"]).read_bytes())
        validate_uf2_artifact(variant, bytes(payload))
        payload[0] = 0
        with self.assertRaisesRegex(ValueError, "invalid magic"):
            validate_uf2_artifact(variant, bytes(payload))

    def test_release_boundary_binds_nordic_recovery_to_dfu_application(self) -> None:
        target = next(
            target
            for target in self.fixture.manifest["targets"]
            if target["board_slug"] == "t1000-e"
        )
        nrf_serial_dfu = target["nrf_serial_dfu"]
        application = (self.fixture.root / nrf_serial_dfu["application"]["path"]).read_bytes()
        recovery = (self.fixture.root / nrf_serial_dfu["recovery"]["artifact"]["path"]).read_bytes()
        validate_nrf_serial_dfu_recovery_artifact(target, application, recovery)
        tampered = bytes([application[0] ^ 0xFF]) + application[1:]
        with self.assertRaisesRegex(ValueError, "disagrees with the exact DFU application"):
            validate_nrf_serial_dfu_recovery_artifact(target, tampered, recovery)

    def test_embedded_firmware_cannot_carry_the_hosted_source_archive(self) -> None:
        target = next(
            target
            for target in self.fixture.manifest["targets"]
            if target["board_slug"] == "heltec-v4"
        )
        application = next(
            part for part in target["parts"] if part["kind"] == "application"
        )
        application_path = self.fixture.root / application["path"]
        source_archive = (self.fixture.root / "website" / "source.zip").read_bytes()
        application_path.write_bytes(application_path.read_bytes() + source_archive)
        application["size"] = application_path.stat().st_size
        application["sha256"] = sha256(application_path)
        hosted = self.fixture.root / "website" / "releases" / VERSION / application["path"]
        hosted.write_bytes(application_path.read_bytes())
        write_json(self.fixture.manifest_path, self.fixture.manifest)

        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not embed source.zip", result.stderr)

    def test_hosted_source_snapshot_is_bound_to_the_exact_commit(self) -> None:
        archive = self.fixture.root / "website" / "source.zip"
        archive.write_bytes(archive.read_bytes() + b"tampered")
        checksum = self.fixture.root / "website" / "source.zip.sha256"
        checksum.write_text(
            f"{sha256(archive)}  source.zip\n",
            encoding="utf-8",
            newline="",
        )
        report_path = self.fixture.root / "metadata" / "reproducibility.json"
        report = json.loads(report_path.read_text(encoding="utf-8"))
        report["payload"] = payload_identity(
            payload_manifest(self.fixture.root, exclude_report=True)
        )
        write_json(report_path, report)
        self.fixture.write_sums()

        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("differs from the exact stamped Git commit", result.stderr)

    def test_hosted_source_snapshot_checksum_cannot_go_stale(self) -> None:
        checksum = self.fixture.root / "website" / "source.zip.sha256"
        checksum.write_text(f"{'0' * 64}  source.zip\n", encoding="utf-8")
        result = self.validate_unsigned()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum is malformed or stale", result.stderr)

    def test_raw_browser_fixture_trust_is_rejected_outside_the_source_archive(self) -> None:
        browser_wasm = (
            self.fixture.root
            / "website"
            / "browser-node-playground-console"
            / "pkg"
            / "prns_wasm_bg.wasm"
        )
        original = browser_wasm.read_bytes()
        fixture_key = (
            ROOT
            / "docs"
            / "website"
            / "web-flasher"
            / "browser"
            / "fixtures"
            / "signed-candidate"
            / "minisign.pub"
        ).read_bytes().splitlines()[1]
        for trust_material in (
            b"PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1",
            fixture_key,
        ):
            with self.subTest(trust_material=trust_material):
                browser_wasm.write_bytes(original + b" " + trust_material)
                result = self.validate_unsigned()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(
                    "hosted website contains browser-test trust material",
                    result.stderr,
                )
        browser_wasm.write_bytes(original)

    def test_fake_signer_injection_signs_documents_and_hosted_copies(self) -> None:
        result = self.sign_candidate()
        self.assertEqual(result.returncode, 0, result.stderr)
        for document in (
            self.fixture.manifest_path,
            self.fixture.channel_path,
            self.fixture.root / "SHA256SUMS.txt",
        ):
            self.assertTrue(Path(f"{document}.minisig").is_file())
        self.assertEqual(
            (self.fixture.root / "website" / "releases" / VERSION / "flash-manifest.json.minisig").read_bytes(),
            Path(f"{self.fixture.manifest_path}.minisig").read_bytes(),
        )
        rerun = self.sign_candidate()
        self.assertNotEqual(rerun.returncode, 0)
        self.assertIn("existing signature", rerun.stderr)

    def test_signed_candidate_packaging_is_deterministic(self) -> None:
        self.assertEqual(self.sign_candidate().returncode, 0)
        first = self.workspace / "first.tar.gz"
        second = self.workspace / "second.tar.gz"
        self.assertEqual(
            run_script("package-flasher-candidate.py", self.fixture.root, first).returncode, 0
        )
        for path in self.fixture.root.rglob("*"):
            os.utime(path, (1_900_000_000, 1_900_000_000), follow_symlinks=False)
        self.assertEqual(
            run_script("package-flasher-candidate.py", self.fixture.root, second).returncode, 0
        )
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_minisign_trusted_comment_is_bound_to_document_hash(self) -> None:
        signer = (SCRIPTS / "sign-flasher-document.sh").read_text(encoding="utf-8")
        self.assertIn("prns-release-sha256:${document_sha256}", signer)
        self.assertNotIn("timestamp:", signer)

    def test_attestation_requires_exact_canonical_name_and_digest_pair(self) -> None:
        subject = self.workspace / "artifact.bin"
        subject.write_bytes(b"same digest, wrong name")
        checksums = self.workspace / "subjects.sha256"
        generated = run_script(
            "write-flasher-attestation-checksums.py",
            "--subject",
            "canonical/artifact.bin",
            subject,
            "--output",
            checksums,
        )
        self.assertEqual(generated.returncode, 0, generated.stderr)
        self.assertEqual(
            checksums.read_text(encoding="utf-8"),
            f"{sha256(subject)}  canonical/artifact.bin\n",
        )
        statement = {
            "_type": "https://in-toto.io/Statement/v1",
            "subject": [
                {
                    "name": "wrong/artifact.bin",
                    "digest": {"sha256": sha256(subject)},
                }
            ],
            "predicateType": "https://slsa.dev/provenance/v1",
            "predicate": {},
        }
        bundle = self.workspace / "attestation.json"
        write_json(
            bundle,
            {
                "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
                "dsseEnvelope": {
                    "payloadType": "application/vnd.in-toto+json",
                    "payload": base64.b64encode(json.dumps(statement).encode()).decode(),
                    "signatures": [{"sig": "fixture"}],
                },
            },
        )
        result = run_script(
            "record-flasher-attestation.py",
            "--bundle",
            bundle,
            "--required-subject",
            "canonical/artifact.bin",
            subject,
            "--repository",
            REPOSITORY,
            "--workflow-ref",
            f"{REPOSITORY}/.github/workflows/flasher-sign.yml@refs/heads/main",
            "--workflow-sha",
            SOURCE_COMMIT,
            "--workflow-run-id",
            "77",
            "--attestation-id",
            "12345",
            "--attestation-url",
            f"https://github.com/{REPOSITORY}/attestations/12345",
            "--output",
            self.workspace / "metadata.json",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exact canonical inputs", result.stderr)

    def test_manifest_artifact_listing_covers_every_transport(self) -> None:
        paths = run_script(
            "list-flasher-manifest-artifacts.py", self.fixture.manifest_path
        )
        self.assertEqual(paths.returncode, 0, paths.stderr)
        expected = sorted(
            path.relative_to(self.fixture.root).as_posix()
            for path in self.fixture.firmware_paths
        )
        self.assertEqual(paths.stdout.splitlines(), expected)

        identities = run_script(
            "list-flasher-manifest-artifacts.py",
            self.fixture.manifest_path,
            "--format",
            "identities",
        )
        self.assertEqual(identities.returncode, 0, identities.stderr)
        self.assertEqual(
            identities.stdout.splitlines(),
            [
                f"{relative}\t{(self.fixture.root / relative).stat().st_size}\t"
                f"{sha256(self.fixture.root / relative)}"
                for relative in expected
            ],
        )

    def test_v037_archive_coverage_is_exact_and_one_time(self) -> None:
        script = SCRIPTS / "flasher-release-record.py"
        spec = importlib.util.spec_from_file_location("flasher_release_record", script)
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        exception = module.V037_ARCHIVE_COVERAGE
        archive = {
            "name": "prns-flasher-candidate-v0.3.7-signed.tar.gz",
            "sha256": exception["signed_bundle_sha256"],
        }
        attested = {(archive["name"], archive["sha256"])}
        coverage = module.archive_coverage(
            version=exception["version"],
            source_commit=exception["source_commit"],
            signed_bundle=archive,
            attestation_bundle_sha256=exception["attestation_bundle_sha256"],
            attestation_workflow_run_id=exception["attestation_workflow_run_id"],
            attested_subjects=attested,
            missing=set(exception["subjects"]),
            unexpected=set(),
        )
        self.assertIsNotNone(coverage)
        self.assertEqual(len(coverage["subjects"]), 7)

        wrong_missing = set(exception["subjects"])
        wrong_missing.pop()
        self.assertIsNone(
            module.archive_coverage(
                version=exception["version"],
                source_commit=exception["source_commit"],
                signed_bundle=archive,
                attestation_bundle_sha256=exception["attestation_bundle_sha256"],
                attestation_workflow_run_id=exception["attestation_workflow_run_id"],
                attested_subjects=attested,
                missing=wrong_missing,
                unexpected=set(),
            )
        )

    def test_candidate_run_must_be_successful_default_branch_provenance(self) -> None:
        run_document = {
            "id": 42,
            "repository": {"full_name": REPOSITORY},
            "head_repository": {"full_name": REPOSITORY},
            "path": ".github/workflows/flasher-candidate.yml",
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": SOURCE_COMMIT,
            "run_attempt": 1,
        }
        run_json = self.workspace / "run.json"
        output = self.workspace / "run-identity.json"
        write_json(run_json, run_document)
        result = run_script(
            "validate-flasher-candidate-run.py",
            "--run-json",
            run_json,
            "--manifest",
            self.fixture.manifest_path,
            "--expected-run-id",
            "42",
            "--repository",
            REPOSITORY,
            "--default-branch",
            "main",
            "--output",
            output,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        run_document["head_branch"] = "feature"
        write_json(run_json, run_document)
        rejected = run_script(
            "validate-flasher-candidate-run.py",
            "--run-json",
            run_json,
            "--manifest",
            self.fixture.manifest_path,
            "--expected-run-id",
            "42",
            "--repository",
            REPOSITORY,
            "--default-branch",
            "main",
            "--output",
            output,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("default branch", rejected.stderr)

    def make_release_evidence(
        self, *, include_firmware_attestations: bool = True
    ) -> tuple[Path, Path, Path, Path, Path, Path]:
        self.assertEqual(self.sign_candidate().returncode, 0)
        signed_bundle = self.workspace / f"prns-flasher-candidate-v{VERSION}-signed.tar.gz"
        result = run_script(
            "package-flasher-candidate.py", self.fixture.root, signed_bundle
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        subject_paths = [
            (signed_bundle.name, signed_bundle),
            *(
                (f"cli/{archive.name}", archive)
                for archive in self.fixture.cli_archives
            ),
        ]
        if include_firmware_attestations:
            subject_paths.extend(
                (path.relative_to(self.fixture.root).as_posix(), path)
                for path in self.fixture.firmware_paths
            )
        statement = {
            "_type": "https://in-toto.io/Statement/v1",
            "subject": [
                {"name": name, "digest": {"sha256": sha256(path)}}
                for name, path in subject_paths
            ],
            "predicateType": "https://slsa.dev/provenance/v1",
            "predicate": {},
        }
        bundle = self.workspace / f"prns-flasher-attestation-v{VERSION}.json"
        write_json(
            bundle,
            {
                "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
                "dsseEnvelope": {
                    "payloadType": "application/vnd.in-toto+json",
                    "payload": base64.b64encode(json.dumps(statement).encode()).decode(),
                    "signatures": [{"sig": "fixture"}],
                },
            },
        )
        metadata = self.workspace / f"prns-flasher-attestation-v{VERSION}.metadata.json"
        arguments: list[object] = ["--bundle", bundle]
        for name, subject in subject_paths:
            arguments.extend(("--required-subject", name, subject))
        arguments.extend(
            (
                "--repository",
                REPOSITORY,
                "--workflow-ref",
                f"{REPOSITORY}/.github/workflows/flasher-sign.yml@refs/heads/main",
                "--workflow-sha",
                SOURCE_COMMIT,
                "--workflow-run-id",
                "77",
                "--attestation-id",
                "12345",
                "--attestation-url",
                f"https://github.com/{REPOSITORY}/attestations/12345",
                "--output",
                metadata,
            )
        )
        result = run_script("record-flasher-attestation.py", *arguments)
        self.assertEqual(result.returncode, 0, result.stderr)
        acceptance = self.workspace / f"acceptance-v{VERSION}.json"
        write_json(
            acceptance,
            {
                "schema": 4,
                "candidate": {
                    "version": VERSION,
                    "channel": "stable",
                    "source_commit": SOURCE_COMMIT,
                    "signing_key_id": KEY_ID,
                    "manifest_sha256": sha256(self.fixture.manifest_path),
                    "manifest_signature_sha256": sha256(
                        Path(f"{self.fixture.manifest_path}.minisig")
                    ),
                    "signed_candidate_sha256": sha256(signed_bundle),
                    "prerelease_published_at": "2026-07-20T12:00:00Z",
                },
            },
        )
        result = run_script(
            "sign-flasher-document.sh",
            acceptance,
            self.secret,
            environment=self.environment,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        candidate_run = self.workspace / f"prns-flasher-candidate-run-v{VERSION}.json"
        write_json(
            candidate_run,
            {
                "schema": 1,
                "repository": REPOSITORY,
                "workflow_path": ".github/workflows/flasher-candidate.yml",
                "workflow_run_id": 42,
                "workflow_run_attempt": 3,
                "source_commit": SOURCE_COMMIT,
            },
        )
        qualification_evidence = (
            self.workspace / f"qualification-evidence-v{VERSION}.tar.gz"
        )
        qualification_evidence.write_bytes(b"fixture qualification evidence\n")
        self.public_review_evidence = (
            self.workspace / f"public-review-v{VERSION}-run-77-attempt-2.json"
        )
        write_json(
            self.public_review_evidence,
            {
                "schema": 2,
                "repository": REPOSITORY,
                "workflow_path": ".github/workflows/flasher-sign.yml",
                "workflow_sha": SOURCE_COMMIT,
                "workflow_run_id": 77,
                "workflow_run_attempt": 2,
                "workflow_job_id": 88,
                "version": VERSION,
                "source_commit": SOURCE_COMMIT,
                "signed_candidate_sha256": sha256(signed_bundle),
                "manifest_sha256": sha256(self.fixture.manifest_path),
                "prerelease_published_at": "2026-07-20T12:00:00Z",
                "approved_at": "2026-07-20T12:02:00Z",
            },
        )
        return (
            signed_bundle,
            bundle,
            metadata,
            acceptance,
            candidate_run,
            qualification_evidence,
        )

    def test_release_record_binds_candidate_acceptance_audit_and_attestation(self) -> None:
        (
            signed_bundle,
            bundle,
            metadata,
            acceptance,
            candidate_run,
            qualification_evidence,
        ) = self.make_release_evidence()
        record = self.workspace / f"release-record-v{VERSION}.json"
        common: list[object] = [
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--qualification-evidence",
            qualification_evidence,
            "--public-review-evidence",
            self.public_review_evidence,
            "--prerelease-published-at",
            "2026-07-20T12:00:00Z",
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
        ]
        created = run_script("flasher-release-record.py", "create", *common, "--output", record)
        self.assertEqual(created.returncode, 0, created.stderr)
        verified = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertEqual(verified.returncode, 0, verified.stderr)
        record_value = json.loads(record.read_text(encoding="utf-8"))
        self.assertEqual(
            record_value["release"]["prerelease_published_at"],
            "2026-07-20T12:00:00Z",
        )
        self.assertEqual(
            record_value["qualification_evidence"],
            {
                "name": qualification_evidence.name,
                "size": qualification_evidence.stat().st_size,
                "sha256": sha256(qualification_evidence),
            },
        )
        self.assertEqual(
            record_value["public_review"]["evidence"]["sha256"],
            sha256(self.public_review_evidence),
        )
        audit = self.fixture.root / "audit" / "release-audit-evidence.md"
        audit_bytes = audit.read_bytes()
        audit.write_bytes(audit_bytes + b"not in the signed archive\n")
        rejected_candidate = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(rejected_candidate.returncode, 0)
        self.assertIn("candidate directory bytes differ", rejected_candidate.stderr)
        audit.write_bytes(audit_bytes)

        review_bytes = self.public_review_evidence.read_bytes()
        self.public_review_evidence.write_bytes(review_bytes + b" ")
        rejected_review = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(rejected_review.returncode, 0)
        self.assertIn("release record does not match", rejected_review.stderr)
        self.public_review_evidence.write_bytes(review_bytes)
        qualification_bytes = qualification_evidence.read_bytes()
        qualification_evidence.write_bytes(qualification_bytes + b"tampered")
        rejected_qualification = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(rejected_qualification.returncode, 0)
        self.assertIn("release record does not match", rejected_qualification.stderr)
        qualification_evidence.write_bytes(qualification_bytes)
        acceptance.write_text(acceptance.read_text() + " ", encoding="utf-8")
        rejected = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("release record does not match", rejected.stderr)

    def test_release_record_rejects_candidate_run_tamper_and_identity_mismatch(self) -> None:
        (
            signed_bundle,
            bundle,
            metadata,
            acceptance,
            candidate_run,
            qualification_evidence,
        ) = self.make_release_evidence()
        record = self.workspace / f"release-record-v{VERSION}.json"
        common: list[object] = [
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--qualification-evidence",
            qualification_evidence,
            "--public-review-evidence",
            self.public_review_evidence,
            "--prerelease-published-at",
            "2026-07-20T12:00:00Z",
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
        ]
        created = run_script("flasher-release-record.py", "create", *common, "--output", record)
        self.assertEqual(created.returncode, 0, created.stderr)

        run_evidence = json.loads(candidate_run.read_text(encoding="utf-8"))
        run_evidence["workflow_run_attempt"] = 4
        write_json(candidate_run, run_evidence)
        tampered = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(tampered.returncode, 0)
        self.assertIn("release record does not match", tampered.stderr)

        run_evidence["source_commit"] = "c" * 40
        write_json(candidate_run, run_evidence)
        mismatched = run_script(
            "flasher-release-record.py", "verify", *common, "--release-record", record
        )
        self.assertNotEqual(mismatched.returncode, 0)
        self.assertIn("source commit differs", mismatched.stderr)

    def test_unified_suite_review_can_follow_the_original_flasher_attestation(self) -> None:
        (
            signed_bundle,
            bundle,
            metadata,
            acceptance,
            candidate_run,
            qualification_evidence,
        ) = self.make_release_evidence()
        review = json.loads(self.public_review_evidence.read_text(encoding="utf-8"))
        review["workflow_path"] = ".github/workflows/suite-sign.yml"
        review["workflow_run_id"] = 91
        suite_review = (
            self.workspace
            / f"public-review-v{VERSION}-run-91-attempt-2.json"
        )
        write_json(suite_review, review)
        record = self.workspace / f"flasher-release-record-v{VERSION}.json"

        created = run_script(
            "flasher-release-record.py",
            "create",
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--qualification-evidence",
            qualification_evidence,
            "--public-review-evidence",
            suite_review,
            "--prerelease-published-at",
            "2026-07-20T12:00:00Z",
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
            "--output",
            record,
        )

        self.assertEqual(created.returncode, 0, created.stderr)
        value = json.loads(record.read_text(encoding="utf-8"))
        self.assertEqual(
            value["public_review"]["workflow_path"],
            ".github/workflows/suite-sign.yml",
        )
        self.assertEqual(
            value["attestation"]["metadata"]["workflow_run_id"], 77
        )
        self.assertEqual(value["public_review"]["workflow_run_id"], 91)

    def test_release_record_requires_provenance_for_every_firmware_payload(self) -> None:
        signed_bundle, bundle, metadata, acceptance, candidate_run, qualification_evidence = (
            self.make_release_evidence(include_firmware_attestations=False)
        )
        record = self.workspace / f"release-record-v{VERSION}.json"
        result = run_script(
            "flasher-release-record.py",
            "create",
            "--candidate",
            self.fixture.root,
            "--candidate-run",
            candidate_run,
            "--signed-bundle",
            signed_bundle,
            "--acceptance",
            acceptance,
            "--acceptance-source-commit",
            ACCEPTANCE_COMMIT,
            "--qualification-evidence",
            qualification_evidence,
            "--public-review-evidence",
            self.public_review_evidence,
            "--prerelease-published-at",
            "2026-07-20T12:00:00Z",
            "--attestation-bundle",
            bundle,
            "--attestation-metadata",
            metadata,
            "--repository",
            REPOSITORY,
            "--output",
            record,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("attestation subjects differ from release paths", result.stderr)

    def test_public_review_gate_uses_release_time_and_exact_commit(self) -> None:
        script = SCRIPTS / "validate-flasher-prerelease.py"
        spec = importlib.util.spec_from_file_location("validate_flasher_prerelease", script)
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        now = datetime(2026, 7, 21, 12, 0, tzinfo=timezone.utc)
        release_json = self.workspace / "release.json"
        write_json(
            release_json,
            {
                "isDraft": False,
                "isPrerelease": True,
                "tagName": f"v{VERSION}",
                "targetCommitish": SOURCE_COMMIT,
                "publishedAt": (now - timedelta(minutes=1)).isoformat(),
            },
        )
        arguments = argparse.Namespace(
            release_json=release_json,
            version=VERSION,
            source_commit=SOURCE_COMMIT,
            allow_promoted=False,
        )
        module.validate(arguments, now=now)
        release = json.loads(release_json.read_text())
        release["publishedAt"] = (now + timedelta(seconds=1)).isoformat()
        write_json(release_json, release)
        with self.assertRaisesRegex(ValueError, "future"):
            module.validate(arguments, now=now)

        release["publishedAt"] = (now - timedelta(minutes=1)).isoformat()
        release["isPrerelease"] = False
        write_json(release_json, release)
        with self.assertRaisesRegex(ValueError, "unless exact promotion is resuming"):
            module.validate(arguments, now=now)
        arguments.allow_promoted = True
        module.validate(arguments, now=now)

    def test_public_release_asset_inventory_and_candidate_bytes_are_exact(self) -> None:
        self.assertEqual(self.sign_candidate().returncode, 0)
        assets = self.workspace / "release-assets"
        assets.mkdir()
        candidate_assets = [
            self.fixture.root / "SHA256SUMS.txt",
            Path(f"{self.fixture.root / 'SHA256SUMS.txt'}.minisig"),
            self.fixture.manifest_path,
            Path(f"{self.fixture.manifest_path}.minisig"),
            self.fixture.channel_path,
            Path(f"{self.fixture.channel_path}.minisig"),
            self.fixture.root / "minisign.pub",
            self.fixture.root / "cli" / "install.sh",
            self.fixture.root / "cli" / "install.ps1",
            self.fixture.root / "cli" / "README.md",
            self.fixture.root / "qualification" / "QUALIFICATION.md",
            self.fixture.root / "qualification" / "create-flasher-acceptance.py",
            self.fixture.root / "qualification" / "validate-flasher-acceptance.py",
            self.fixture.root / "qualification" / "flasher_acceptance_contract.py",
            self.fixture.root / "qualification" / "flasher_hotfix.py",
            self.fixture.root / "qualification" / "flasher_manifest.py",
            self.fixture.root / "qualification" / "serve-flasher-candidate.py",
            self.fixture.root / "qualification" / "verify-flasher-candidate-files.py",
            self.fixture.root / "qualification" / "validate-flasher-tester-roster.py",
            self.fixture.root / "qualification" / "flasher_tester_roster.py",
            self.fixture.root
            / "qualification"
            / "package-flasher-qualification-evidence.py",
            self.fixture.root / "qualification" / "tester-roster.json",
            self.fixture.root / "audit" / "release-audit-evidence.md",
            self.fixture.root / "metadata" / "build.json",
            self.fixture.root / "metadata" / "sparse-sizes.json",
            self.fixture.root / "metadata" / "reproducibility.json",
            self.fixture.root / "metadata" / "release-history.json",
            *self.fixture.cli_archives,
        ]
        for source in candidate_assets:
            shutil.copyfile(source, assets / source.name)
        for name in (
            f"prns-flasher-candidate-v{VERSION}-signed.tar.gz",
            f"prns-flasher-candidate-run-v{VERSION}.json",
            f"prns-flasher-attestation-v{VERSION}.json",
            f"prns-flasher-attestation-v{VERSION}.metadata.json",
            f"acceptance-v{VERSION}.json",
            f"acceptance-v{VERSION}.json.minisig",
            f"qualification-evidence-v{VERSION}.tar.gz",
            f"release-record-v{VERSION}.json",
            f"release-record-v{VERSION}.json.minisig",
            f"flasher-release-record-v{VERSION}.json",
            f"flasher-release-record-v{VERSION}.json.minisig",
        ):
            (assets / name).write_text(f"fixture {name}\n", encoding="utf-8")
        workflow_run_id = 77
        write_json(
            assets / f"prns-flasher-attestation-v{VERSION}.metadata.json",
            {"repository": REPOSITORY, "workflow_run_id": workflow_run_id},
        )
        signed_bundle = assets / f"prns-flasher-candidate-v{VERSION}-signed.tar.gz"
        write_json(
            assets
            / f"public-review-v{VERSION}-run-{workflow_run_id}-attempt-2.json",
            {
                "schema": 2,
                "repository": REPOSITORY,
                "workflow_path": ".github/workflows/flasher-sign.yml",
                "workflow_sha": SOURCE_COMMIT,
                "workflow_run_id": workflow_run_id,
                "workflow_run_attempt": 2,
                "workflow_job_id": 88,
                "version": VERSION,
                "source_commit": SOURCE_COMMIT,
                "signed_candidate_sha256": sha256(signed_bundle),
                "manifest_sha256": sha256(self.fixture.manifest_path),
                "prerelease_published_at": "2026-07-20T12:00:00Z",
                "approved_at": "2026-07-20T12:02:00Z",
            },
        )
        arguments = (
            "--candidate",
            self.fixture.root,
            "--assets",
            assets,
            "--version",
            VERSION,
        )
        verified = run_script("verify-flasher-release-assets.py", *arguments)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        remote_inventory = self.workspace / "release-assets.json"
        inventory = [
            {
                "name": path.name,
                "size": path.stat().st_size,
                "digest": f"sha256:{sha256(path)}",
            }
            for path in sorted(assets.iterdir(), key=lambda path: path.name)
        ]
        write_json(remote_inventory, inventory)
        verified_remote = run_script(
            "verify-flasher-release-assets.py",
            *arguments,
            "--remote-inventory",
            remote_inventory,
        )
        self.assertEqual(verified_remote.returncode, 0, verified_remote.stderr)
        inventory[0]["digest"] = f"sha256:{'0' * 64}"
        write_json(remote_inventory, inventory)
        rejected_remote = run_script(
            "verify-flasher-release-assets.py",
            *arguments,
            "--remote-inventory",
            remote_inventory,
        )
        self.assertNotEqual(rejected_remote.returncode, 0)
        self.assertIn("downloaded asset bytes differ", rejected_remote.stderr)

        readme = assets / "README.md"
        expected_readme = readme.read_bytes()
        readme.write_bytes(b"tampered")
        tampered = run_script("verify-flasher-release-assets.py", *arguments)
        self.assertNotEqual(tampered.returncode, 0)
        self.assertIn("asset bytes differ", tampered.stderr)
        readme.write_bytes(expected_readme)

        (assets / "unexpected.bin").write_bytes(b"unexpected")
        unexpected = run_script("verify-flasher-release-assets.py", *arguments)
        self.assertNotEqual(unexpected.returncode, 0)
        self.assertIn(
            "outside both the signed candidate and the signed suite custody inventory",
            unexpected.stderr,
        )
        (assets / "unexpected.bin").unlink()

        removed = assets / "install.sh"
        expected_install = removed.read_bytes()
        removed.unlink()
        missing = run_script("verify-flasher-release-assets.py", *arguments)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("missing signed release assets", missing.stderr)
        removed.write_bytes(expected_install)

        suite_sums = assets / "SHA256SUMS.txt"
        suite_signature = assets / "SHA256SUMS.txt.minisig"
        candidate_sums = suite_sums.read_bytes()
        candidate_signature = suite_signature.read_bytes()
        suite_extra = assets / f"prns-host-sdk-v{VERSION}.tar.gz"
        suite_extra.write_bytes(b"suite asset")

        def sign_suite_inventory(lines: list[str]) -> None:
            suite_sums.write_text("".join(lines), encoding="utf-8")
            signed = subprocess.run(
                [str(self.signer), "-S", "-m", str(suite_sums), "-x", str(suite_signature)],
                capture_output=True,
            )
            self.assertEqual(signed.returncode, 0)

        inventory_lines = [
            f"{sha256(suite_extra)}  {suite_extra.name}\n",
            f"{sha256(assets / 'install.sh')}  install.sh\n",
            f"{sha256(assets / 'flash-manifest.json')}  flash-manifest.json\n",
        ]
        sign_suite_inventory(inventory_lines)
        suite_verified = run_script(
            "verify-flasher-release-assets.py", *arguments, environment=self.environment
        )
        self.assertEqual(suite_verified.returncode, 0, suite_verified.stderr)

        contradicting_lines = list(inventory_lines)
        contradicting_lines[1] = f"{'0' * 64}  install.sh\n"
        sign_suite_inventory(contradicting_lines)
        contradicted = run_script(
            "verify-flasher-release-assets.py", *arguments, environment=self.environment
        )
        self.assertNotEqual(contradicted.returncode, 0)
        self.assertIn("contradicts the signed candidate", contradicted.stderr)

        sign_suite_inventory(inventory_lines)
        suite_extra.write_bytes(b"tampered suite asset")
        tampered_extra = run_script(
            "verify-flasher-release-assets.py", *arguments, environment=self.environment
        )
        self.assertNotEqual(tampered_extra.returncode, 0)
        self.assertIn(
            "outside both the signed candidate and the signed suite custody inventory",
            tampered_extra.stderr,
        )

        refusing_signer = self.workspace / "refusing-minisign"
        refusing_signer.write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        refusing_signer.chmod(refusing_signer.stat().st_mode | stat.S_IXUSR)
        refusing_environment = dict(self.environment)
        refusing_environment["PRNS_MINISIGN_BIN"] = str(refusing_signer)
        rejected_signature = run_script(
            "verify-flasher-release-assets.py", *arguments, environment=refusing_environment
        )
        self.assertNotEqual(rejected_signature.returncode, 0)
        self.assertIn(
            "suite custody inventory signature verification failed",
            rejected_signature.stderr,
        )

        suite_extra.unlink()
        suite_sums.write_bytes(candidate_sums)
        suite_signature.write_bytes(candidate_signature)

    def test_release_asset_contract_tracks_candidate_manifest_schema(self) -> None:
        self.assertEqual(self.sign_candidate().returncode, 0)
        script = SCRIPTS / "verify-flasher-release-assets.py"
        spec = importlib.util.spec_from_file_location(
            "verify_flasher_release_assets", script
        )
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        schema_three = module.expected_candidate_assets(self.fixture.root, VERSION)
        self.assertIn("flasher_manifest.py", schema_three)

        self.fixture.manifest["schema"] = 2
        write_json(self.fixture.manifest_path, self.fixture.manifest)
        (self.fixture.root / "qualification" / "flasher_manifest.py").unlink()
        schema_two = module.expected_candidate_assets(self.fixture.root, VERSION)
        self.assertNotIn("flasher_manifest.py", schema_two)

        self.fixture.manifest["schema"] = 3
        write_json(self.fixture.manifest_path, self.fixture.manifest)
        with self.assertRaisesRegex(ValueError, "flasher_manifest.py"):
            module.expected_candidate_assets(self.fixture.root, VERSION)

        self.fixture.manifest["schema"] = 4
        write_json(self.fixture.manifest_path, self.fixture.manifest)
        with self.assertRaisesRegex(ValueError, "schema is unsupported"):
            module.expected_candidate_assets(self.fixture.root, VERSION)

    def test_hotfix_asset_contract_does_not_require_a_suite_release_record(self) -> None:
        script = SCRIPTS / "verify-flasher-release-assets.py"
        spec = importlib.util.spec_from_file_location(
            "verify_flasher_release_assets", script
        )
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        suite_record = f"release-record-v{VERSION}.json"
        flasher_record = f"flasher-release-record-v{VERSION}.json"
        regular = module.required_custody_assets(self.fixture.root, VERSION)
        self.assertIn(suite_record, regular)
        self.assertIn(flasher_record, regular)

        write_json(self.fixture.root / "metadata" / "hotfix.json", {})
        hotfix = module.required_custody_assets(self.fixture.root, VERSION)
        self.assertNotIn(suite_record, hotfix)
        self.assertNotIn(f"{suite_record}.minisig", hotfix)
        self.assertIn(flasher_record, hotfix)
        self.assertIn(f"{flasher_record}.minisig", hotfix)

    def test_historical_candidate_need_not_contain_the_new_hotfix_helper(self) -> None:
        self.assertEqual(self.sign_candidate().returncode, 0)
        script = SCRIPTS / "verify-flasher-release-assets.py"
        spec = importlib.util.spec_from_file_location(
            "verify_flasher_release_assets", script
        )
        if spec is None or spec.loader is None:
            self.fail(f"could not import {script}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        helper = self.fixture.root / "qualification" / "flasher_hotfix.py"
        helper.unlink()
        historical = module.expected_candidate_assets(self.fixture.root, VERSION)
        self.assertNotIn("flasher_hotfix.py", historical)

        write_json(self.fixture.root / "metadata" / "hotfix.json", {})
        with self.assertRaisesRegex(ValueError, "flasher_hotfix.py"):
            module.expected_candidate_assets(self.fixture.root, VERSION)

    def test_workflows_preserve_exact_candidate_custody_boundaries(self) -> None:
        candidate = (ROOT / ".github/workflows/flasher-candidate.yml").read_text()
        signing = (ROOT / ".github/workflows/flasher-sign.yml").read_text()
        evidence = (ROOT / ".github/workflows/flasher-finalize-evidence.yml").read_text()
        promotion = (ROOT / ".github/workflows/flasher-promote.yml").read_text()
        rollback = (ROOT / ".github/workflows/flasher-rollback.yml").read_text()
        suite_promotion = (ROOT / ".github/workflows/suite-promote.yml").read_text()
        for workflow in (
            candidate,
            signing,
            evidence,
            promotion,
            rollback,
            suite_promotion,
        ):
            self.assertNotIn(".targets[].parts", workflow)
        self.assertIn("release manifest artifacts", signing)
        self.assertIn("release manifest artifacts", promotion)
        self.assertIn(".subjects[]", candidate)
        self.assertNotIn("gh release create", candidate)
        self.assertIn("candidate_run_id:", signing)
        self.assertIn("unsigned_bundle_sha256:", signing)
        self.assertIn("environment: release-signing", signing)
        self.assertIn("name: Approve protected public release", signing)
        self.assertIn("environment: public-release", signing)
        self.assertIn("./tools/prns release public-review -- create", signing)
        self.assertIn(
            "Publish immutable attempt-specific public-review evidence", signing
        )
        self.assertIn(
            "actions/runs/${GITHUB_RUN_ID}/attempts/${GITHUB_RUN_ATTEMPT}", signing
        )
        self.assertIn("qualification/tester-roster.json", signing)
        self.assertIn("PRNS_MINISIGN_SECRET_KEY_B64", signing)
        self.assertIn(
            "actions/attest@59d89421af93a897026c735860bf21b6eb4f7b26", signing
        )
        self.assertIn("artifact-metadata: write", signing)
        self.assertIn("prns-flasher-candidate-run-v", signing)
        self.assertIn("--draft", signing)
        self.assertIn("gh release delete \"$tag\" --yes --cleanup-tag", signing)
        self.assertIn("cmp \"$local_asset\" \"$remote\"", signing)
        self.assertIn("--draft=false --prerelease=true", signing)
        for forbidden in (
            "./tools/prns release candidate build",
            "cargo build",
            "npm run",
            "dx build",
        ):
            self.assertNotIn(forbidden, signing)
        self.assertIn("release/acceptance/records/${RELEASE_VERSION}.json", evidence)
        self.assertIn('PYTHONDONTWRITEBYTECODE: "1"', evidence)
        self.assertIn("./tools/prns release record -- create", evidence)
        self.assertIn("./tools/prns release record -- verify", evidence)
        self.assertIn("published-evidence", evidence)
        self.assertIn("cmp \"$local_asset\" \"$remote\"", evidence)
        self.assertIn("--candidate-run", evidence)
        self.assertIn("--public-review-evidence", evidence)
        self.assertIn("Select and verify the exact successful protected public review", evidence)
        self.assertIn(
            "actions/runs/${review_run_id}/attempts/${run_attempt}", evidence
        )
        self.assertIn("./tools/prns release document sign -- \"$record\"", evidence)
        self.assertIn("release_record_sha256:", promotion)
        self.assertIn("./tools/prns release verify --", promotion)
        self.assertIn("--candidate-run", promotion)
        self.assertNotIn("--minimum-hours", promotion)
        self.assertIn("--allow-promoted", promotion)
        self.assertIn("./tools/prns release public-review -- verify", promotion)
        self.assertIn(".public_review.evidence.name", promotion)
        self.assertIn(".public_review.workflow_run_attempt", promotion)
        self.assertIn(
            "actions/runs/${signing_run_id}/attempts/${run_attempt}", promotion
        )
        self.assertNotIn("environment: public-release", promotion)
        self.assertIn("./tools/prns release assets verify --", promotion)
        self.assertLess(
            promotion.index("Verify the complete prerelease asset inventory before deployment"),
            promotion.index("actions/deploy-pages@"),
        )
        self.assertIn("{name, size, digest}", promotion)
        self.assertIn("group: prns-public-pages", promotion)
        self.assertIn("Bind Pages artifacts to this exact verification attempt", promotion)
        self.assertIn("candidate_pages_artifact_name:", promotion)
        self.assertIn("rollback_baseline_pages_artifact_name:", promotion)
        self.assertIn("rollback_baseline_stage_artifact_name:", promotion)
        self.assertIn(
            "artifact_name: ${{ needs.verify.outputs.candidate_pages_artifact_name }}",
            promotion,
        )
        self.assertIn("  restore-baseline-on-failure:", promotion)
        self.assertIn("${{ always() && needs.verify.result == 'success'", promotion)
        self.assertIn(
            "needs: [verify, publish-and-deploy, post-promotion-smoke, mark-promoted]",
            promotion,
        )
        self.assertIn(
            "Compare-and-swap only the failed candidate back to its verified baseline",
            promotion,
        )
        self.assertIn("target/recovery-live-cas/stable.json", promotion)
        self.assertIn(
            "artifact_name: ${{ needs.verify.outputs.rollback_baseline_pages_artifact_name }}",
            promotion,
        )
        self.assertIn("rollback_baseline_asset_inventory_sha256:", promotion)
        self.assertIn("EXPECTED_CURRENT_ASSET_INVENTORY_SHA256", promotion)
        self.assertIn("EXPECTED_BASELINE_ASSET_INVENTORY_SHA256", promotion)
        self.assertIn("verify-live-website", promotion)
        self.assertIn("--prerelease=true --latest=false", promotion)
        site = (ROOT / ".github/workflows/site.yml").read_text()
        self.assertIn("group: prns-public-pages", site)
        self.assertIn("vars.PRNS_PUBLIC_SITE_PROMOTED != 'true'", site)
        self.assertIn("Refuse to overwrite any live signed stable channel", site)
        self.assertIn("probe-stable", site)
        self.assertIn("cmp target/live-site-custody/stable.json", site)
        self.assertIn("steps.custody.outputs.deploy == 'true'", site)
        publish_job = promotion[
            promotion.index("  publish-and-deploy:") : promotion.index("  post-promotion-smoke:")
        ]
        smoke_job = promotion[promotion.index("  post-promotion-smoke:") :]
        self.assertNotIn("PRNS_PUBLIC_SITE_PROMOTED", publish_job)
        self.assertIn("PRNS_PUBLIC_SITE_PROMOTED", smoke_job)
        self.assertLess(
            publish_job.index("actions/deploy-pages@"),
            publish_job.index(
                "Verify deployed signed channel and website before release mutation"
            ),
        )
        self.assertLess(
            publish_job.index(
                "Verify deployed signed channel and website before release mutation"
            ),
            publish_job.index(
                "Mark the verified prerelease stable without replacing assets"
            ),
        )
        self.assertLess(
            smoke_job.index("Prove stable release state"),
            smoke_job.index("PRNS_PUBLIC_SITE_PROMOTED"),
        )
        recovery_job = promotion[promotion.index("  restore-baseline-on-failure:") :]
        self.assertLess(
            recovery_job.index("Compare-and-swap only the failed candidate"),
            recovery_job.index("Redeploy the exact verified baseline Pages artifact"),
        )
        self.assertLess(
            recovery_job.index("Verify every restored website byte"),
            recovery_job.index("Restore baseline release metadata"),
        )
        self.assertLess(
            recovery_job.index(
                'gh release edit "v${ROLLBACK_BASELINE_VERSION}" --latest=true'
            ),
            recovery_job.index(
                'gh release edit "v${RELEASE_VERSION}" --prerelease=true --latest=false'
            ),
        )


if __name__ == "__main__":
    unittest.main()
