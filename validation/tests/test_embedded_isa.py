from __future__ import annotations

import hashlib
import os
import tempfile
import tomllib
import unittest
from pathlib import Path
from typing import cast
from unittest import mock

from validation.hardening.embedded_isa.architecture import (
    ArchitectureAdapterError,
    command_for,
    target_build_for,
)
from validation.hardening.embedded_isa.artifacts import clear, directory as artifact_directory
from validation.hardening.embedded_isa.contract import (
    Compiler,
    HostedPackages,
    INVENTORY_PATH,
    ROOT,
    InventoryError,
    load_inventory,
)
from validation.hardening.embedded_host import HostPlatform
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.run import (
    emulator_executable,
    require_emulator_identity,
)
from validation.hardening.embedded_isa.transcript import concise, parse, require_match


class EmbeddedIsaTests(unittest.TestCase):
    def test_inventory_defines_the_architecture_runners_and_shared_kernel(self) -> None:
        inventory = load_inventory()

        self.assertEqual(inventory.kernel.scenario, "shared-state-machines")
        self.assertEqual(inventory.kernel.completed_scenarios, 2)
        self.assertEqual(inventory.rust_toolchain, "1.96.0")
        self.assertEqual(
            [architecture.identifier for architecture in inventory.architectures],
            ["thumbv7em", "riscv32imac", "xtensa-esp32s3"],
        )
        arm = inventory.architecture_for_suite("embedded-isa-thumbv7em")
        self.assertEqual(arm.rust_target, "thumbv7em-none-eabihf")
        self.assertEqual(arm.compiler, Compiler.UPSTREAM)
        self.assertEqual(arm.emulator.identity.version, "11.1.1")
        riscv = inventory.architecture_for_suite("embedded-isa-riscv32imac")
        self.assertEqual(riscv.rust_target, "riscv32imac-unknown-none-elf")
        self.assertEqual(riscv.compiler, Compiler.UPSTREAM)
        self.assertEqual(riscv.emulator.identity.version, "11.1.1")
        xtensa = inventory.architecture_for_suite("embedded-isa-xtensa-esp32s3")
        self.assertEqual(xtensa.rust_target, "xtensa-esp32s3-none-elf")
        self.assertEqual(xtensa.compiler, Compiler.ESP)
        self.assertEqual(
            xtensa.emulator.identity.banner,
            "QEMU emulator version 9.2.2 (esp_develop_9.2.2_20260417)",
        )
        self.assertIsInstance(xtensa.emulator.acquisition, HostedPackages)
        packages = cast(HostedPackages, xtensa.emulator.acquisition)
        self.assertEqual(
            {package.host for package in packages.packages},
            set(HostPlatform),
        )
        self.assertTrue(
            all(len(package.source_sha256) == 64 for package in packages.packages)
        )
        manifest = tomllib.loads(
            (ROOT / "validation" / "manifest.toml").read_text(encoding="utf-8")
        )
        registered_suites = {suite["id"] for suite in manifest["suite"]}
        self.assertTrue(
            all(
                architecture.suite in registered_suites
                for architecture in inventory.architectures
            )
        )

    def test_arm_adapter_uses_the_cortex_m4_machine(self) -> None:
        command = command_for("thumbv7em", Path("qemu"), Path("kernel"))

        self.assertEqual(
            command,
            (
                "qemu",
                "-machine",
                "mps2-an386",
                "-cpu",
                "cortex-m4",
                "-nographic",
                "-monitor",
                "none",
                "-serial",
                "none",
                "-semihosting-config",
                "enable=on,target=native",
                "-kernel",
                "kernel",
            ),
        )
        with self.assertRaises(ArchitectureAdapterError):
            command_for("unknown-architecture", Path("qemu"), Path("kernel"))

    def test_riscv_adapter_uses_the_virt_rv32imac_machine(self) -> None:
        command = command_for("riscv32imac", Path("qemu"), Path("kernel"))

        self.assertEqual(
            command,
            (
                "qemu",
                "-machine",
                "virt",
                "-cpu",
                "sifive-e31",
                "-m",
                "128M",
                "-bios",
                "none",
                "-nographic",
                "-monitor",
                "none",
                "-serial",
                "none",
                "-semihosting-config",
                "enable=on,target=native",
                "-kernel",
                "kernel",
            ),
        )

    def test_xtensa_adapter_uses_the_esp32s3_cpu_and_semihosting(self) -> None:
        command = command_for("xtensa-esp32s3", Path("qemu"), Path("kernel"))

        self.assertEqual(
            command,
            (
                "qemu",
                "-machine",
                "esp32s3",
                "-cpu",
                "esp32s3",
                "-nographic",
                "-monitor",
                "none",
                "-serial",
                "none",
                "-semihosting-config",
                "enable=on,target=native",
                "-kernel",
                "kernel",
            ),
        )

        build = target_build_for("xtensa-esp32s3")
        self.assertEqual(build.linker, "xtensa-esp32s3-elf-gcc")
        arguments = build.cargo_arguments(
            Path(r"C:\ESP Tools\xtensa-gcc.exe"),
            "xtensa-esp32s3-none-elf",
        )
        self.assertIn("-Zbuild-std=core,alloc", arguments)
        self.assertIn(
            r'target.xtensa-esp32s3-none-elf.linker="C:\\ESP Tools\\xtensa-gcc.exe"',
            arguments,
        )

    def test_transcript_is_length_and_digest_checked(self) -> None:
        events = b"\x01\x00\x03arm"
        digest = hashlib.sha256(events).hexdigest()
        output = (
            f"PRNS_ISA_TRANSCRIPT schema=1 scenarios=2 bytes={len(events)} "
            f"digest={digest} events={events.hex()}\n"
        ).encode()

        transcript = parse(output, 2)

        self.assertEqual(transcript.events, events)
        self.assertEqual(transcript.digest, digest)
        with self.assertRaises(EmbeddedIsaError):
            parse(output.replace(digest.encode(), b"0" * 64), 2)
        with self.assertRaises(EmbeddedIsaError):
            parse(output + output, 2)
        with self.assertRaises(EmbeddedIsaError):
            parse(
                output.replace(f"events={events.hex()}".encode(), b"events=0"),
                2,
            )

        displayed = concise(output)
        self.assertNotIn(events.hex().encode(), displayed)
        self.assertIn(digest.encode(), displayed)

    def test_host_and_target_transcripts_must_match_exactly(self) -> None:
        first_events = b"first"
        second_events = b"second"
        first_digest = hashlib.sha256(first_events).hexdigest()
        second_digest = hashlib.sha256(second_events).hexdigest()
        first = parse(
            (
                f"PRNS_ISA_TRANSCRIPT schema=1 scenarios=2 bytes=5 "
                f"digest={first_digest} events={first_events.hex()}"
            ).encode(),
            2,
        )
        second = parse(
            (
                f"PRNS_ISA_TRANSCRIPT schema=1 scenarios=2 bytes=6 "
                f"digest={second_digest} events={second_events.hex()}"
            ).encode(),
            2,
        )

        with self.assertRaises(EmbeddedIsaError):
            require_match(first, second)

    def test_emulator_version_must_match_the_exact_pin(self) -> None:
        architecture = load_inventory().architectures[0]

        require_emulator_identity(architecture, "QEMU emulator version 11.1.1")
        with self.assertRaises(EmbeddedIsaError):
            require_emulator_identity(architecture, "QEMU emulator version 11.1.2")

        xtensa = load_inventory().architectures[2]
        require_emulator_identity(
            xtensa,
            "QEMU emulator version 9.2.2 (esp_develop_9.2.2_20260417)",
        )
        with self.assertRaises(EmbeddedIsaError):
            require_emulator_identity(xtensa, "QEMU emulator version 9.2.2")

    def test_missing_emulator_points_to_the_readiness_doctor(self) -> None:
        architecture = load_inventory().architectures[0]
        with mock.patch(
            "validation.hardening.embedded_isa.run.shutil.which", return_value=None
        ):
            with self.assertRaisesRegex(
                EmbeddedIsaError, "doctor embedded-assurance"
            ):
                emulator_executable(architecture)

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
                with self.assertRaises(EmbeddedIsaError):
                    artifact_directory()
            self.assertFalse(outside.exists())

    def test_runner_clears_only_owned_architecture_evidence(self) -> None:
        architecture = load_inventory().architectures[0]
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            proof = artifacts / "thumbv7em.assurance.json"
            executable = artifacts / "thumbv7em.elf"
            log = artifacts / "thumbv7em.log"
            unrelated = artifacts / "result.json"
            for path in (proof, executable, log, unrelated):
                path.write_text("evidence", encoding="utf-8")

            clear(architecture, artifacts)

            self.assertFalse(proof.exists())
            self.assertFalse(executable.exists())
            self.assertFalse(log.exists())
            self.assertTrue(unrelated.exists())

    def test_malformed_inventory_checksum_is_rejected(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8")
        malformed = contents.replace(
            "079ffbff8a7111bbc89022107cbabf3bbfd614d5fc9d7cc675991196aca12482",
            "not-a-checksum",
        )
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "embedded-isa.toml"
            inventory.write_text(malformed, encoding="utf-8")

            with self.assertRaises(InventoryError):
                load_inventory(inventory)

    def test_unknown_architecture_compiler_is_rejected(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8")
        malformed = contents.replace(
            'compiler = "upstream"', 'compiler = "unknown"', 1
        )
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "embedded-isa.toml"
            inventory.write_text(malformed, encoding="utf-8")

            with self.assertRaises(InventoryError):
                load_inventory(inventory)

    def test_unknown_architecture_id_is_rejected(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8")
        malformed = contents.replace('id = "thumbv7em"', 'id = "unknown"', 1)
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "embedded-isa.toml"
            inventory.write_text(malformed, encoding="utf-8")

            with self.assertRaises(InventoryError):
                load_inventory(inventory)

    def test_duplicate_emulator_package_host_is_rejected(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8")
        malformed = contents.replace(
            'host = "linux-arm64"', 'host = "linux-amd64"', 1
        )
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "embedded-isa.toml"
            inventory.write_text(malformed, encoding="utf-8")

            with self.assertRaises(InventoryError):
                load_inventory(inventory)

    def test_emulator_package_set_must_cover_every_supported_host(self) -> None:
        contents = INVENTORY_PATH.read_text(encoding="utf-8")
        package = (
            '\n[[architecture.emulator_package]]\n'
            'host = "windows-amd64"\n'
            'source_url = "https://github.com/espressif/qemu/releases/download/'
            'esp-develop-9.2.2-20260417/qemu-xtensa-softmmu-'
            'esp_develop_9.2.2_20260417-x86_64-w64-mingw32.tar.xz"\n'
            'source_sha256 = '
            '"3c483d77f5350a568df1faf4d8dbc82c95d6bc2b826d0d4be910485e0a68ca2a"\n'
        )
        malformed = contents.replace(package, "", 1)
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / "embedded-isa.toml"
            inventory.write_text(malformed, encoding="utf-8")

            with self.assertRaises(InventoryError):
                load_inventory(inventory)


if __name__ == "__main__":
    unittest.main()
