from __future__ import annotations

import hashlib
import os
import tempfile
import tomllib
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from validation.hardening.embedded_host import HostPlatform
from validation.hardening.embedded_platform import artifacts
from validation.hardening.embedded_platform.contract import (
    INVENTORY_PATH,
    ROOT,
    EmulatorKind,
    InventoryError,
    Milestone,
    load_inventory,
)
from validation.hardening.embedded_platform.discovery import description_candidates
from validation.hardening.embedded_platform.error import EmbeddedPlatformError
from validation.hardening.embedded_platform.platform import command_for, nrf52840
from validation.hardening.embedded_platform.run import cargo_command, platform_description


class EmbeddedPlatformTests(unittest.TestCase):
    def test_inventory_defines_the_nrf52840_pilot_and_release_custody(self) -> None:
        inventory = load_inventory()

        self.assertEqual(inventory.rust_toolchain, "1.96.0")
        self.assertEqual(len(inventory.platforms), 1)
        platform = inventory.platform_for_suite("embedded-platform-nrf52840")
        self.assertEqual(platform.identifier, "nrf52840")
        self.assertEqual(platform.architecture, "thumbv7em")
        self.assertEqual(platform.rust_target, "thumbv7em-none-eabihf")
        self.assertEqual(platform.memory_profile, "t-echo-s140-v6")
        self.assertEqual(platform.milestone, Milestone.APPLICATION_ENTRY)
        self.assertEqual(platform.emulator.kind, EmulatorKind.RENODE)
        self.assertEqual(platform.emulator.identity[0], "Renode v1.17.0")
        self.assertEqual(
            {package.host for package in platform.emulator.packages},
            {
                HostPlatform.LINUX_AMD64,
                HostPlatform.LINUX_ARM64,
                HostPlatform.MACOS_ARM64,
                HostPlatform.WINDOWS_AMD64,
            },
        )
        self.assertIsNone(platform.emulator.package_for_host(HostPlatform.MACOS_AMD64))
        manifest = tomllib.loads(
            (ROOT / "validation" / "manifest.toml").read_text(encoding="utf-8")
        )
        suites = {suite["id"] for suite in manifest["suite"]}
        self.assertIn(platform.suite, suites)

    def test_inventory_rejects_architecture_target_mismatch(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8").replace(
            'rust_target = "thumbv7em-none-eabihf"',
            'rust_target = "riscv32imac-unknown-none-elf"',
        )

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "inventory.toml"
            path.write_text(contents, encoding="utf-8")
            with self.assertRaisesRegex(InventoryError, "requires Rust target"):
                load_inventory(path)

    def test_inventory_rejects_duplicate_host_packages(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8").replace(
            'host = "linux-arm64"', 'host = "linux-amd64"'
        )

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "inventory.toml"
            path.write_text(contents, encoding="utf-8")
            with self.assertRaisesRegex(InventoryError, "repeats a host"):
                load_inventory(path)

    def test_renode_adapter_uses_the_pinned_model_and_production_elf(self) -> None:
        platform = load_inventory().platforms[0]
        with tempfile.TemporaryDirectory(prefix="platform pilot ") as directory:
            root = Path(directory)
            executable = root / "production.elf"
            description = root / "nrf52840.repl"
            script = root / "pilot.resc"
            config = root / "pilot.config"
            executable.write_bytes(b"elf")
            description.write_text("cpu: CPU.CortexM @ sysbus", encoding="utf-8")

            command = command_for(
                platform,
                Path("renode"),
                executable,
                description,
                script,
                config,
            )

            self.assertEqual(command[0], "renode")
            source = script.read_text(encoding="utf-8")
            self.assertIn("sysbus LoadELF $bin", source)
            self.assertIn(platform.milestone_symbol, source)
            self.assertIn("platform=nrf52840", source)
            self.assertIn("profile=t-echo-s140-v6", source)
            self.assertIn("platform\\ pilot", source)

    def test_milestone_parser_requires_exactly_one_expected_record(self) -> None:
        platform = load_inventory().platforms[0]
        event = nrf52840.milestone_event(platform)

        self.assertEqual(nrf52840.parse_milestone(b"prefix " + event, platform), event)
        with self.assertRaises(EmbeddedPlatformError):
            nrf52840.parse_milestone(b"", platform)
        with self.assertRaises(EmbeddedPlatformError):
            nrf52840.parse_milestone(event + event, platform)

    def test_offline_model_removes_only_the_remote_svd_decoration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.repl"
            target = root / "target.repl"
            source.write_bytes(b"before\n" + nrf52840.SVD_DIRECTIVE + b"after\n")

            nrf52840.materialize_offline_model(source, target)

            self.assertEqual(target.read_bytes(), b"before\nafter\n")
            source.write_bytes(b"without the directive")
            with self.assertRaises(EmbeddedPlatformError):
                nrf52840.materialize_offline_model(source, target)

    def test_platform_model_must_match_the_inventory_checksum(self) -> None:
        platform = load_inventory().platforms[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "bin" / "renode"
            description = executable.parent / platform.platform_description
            description.parent.mkdir(parents=True)
            executable.write_bytes(b"runner")
            description.write_bytes(b"model")
            expected = hashlib.sha256(b"model").hexdigest()
            matching = replace(platform, platform_description_sha256=expected)

            path, fingerprint = platform_description(matching, executable)

            self.assertEqual(path, description.resolve())
            self.assertEqual(fingerprint, expected)
            with self.assertRaisesRegex(EmbeddedPlatformError, "checksum mismatch"):
                platform_description(platform, executable)

    def test_description_candidates_preserve_priority_without_duplicates(self) -> None:
        executable = Path("/opt/renode/bin/renode")

        candidates = description_candidates(executable, "/opt/renode")

        self.assertEqual(candidates[0], Path("/opt/renode"))
        self.assertEqual(len(candidates), len(set(candidates)))

    def test_platform_build_uses_the_profile_and_pinned_toolchain_contract(self) -> None:
        platform = load_inventory().platforms[0]

        command = cargo_command(platform, "1.96.0")

        self.assertIn("+1.96.0", command)
        self.assertIn("--locked", command)
        self.assertIn("platform-nrf52840", command)
        self.assertIn("thumbv7em-none-eabihf", command)

    def test_artifact_directory_is_validated_before_creation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "artifacts"
            outside = temporary / "outside"
            with mock.patch.dict(
                os.environ,
                {
                    "PRNS_VALIDATION_ARTIFACT_ROOT": str(root),
                    "PRNS_VALIDATION_ARTIFACT_DIR": str(outside),
                },
            ):
                with self.assertRaises(EmbeddedPlatformError):
                    artifacts.directory()
            self.assertFalse(outside.exists())


if __name__ == "__main__":
    unittest.main()
