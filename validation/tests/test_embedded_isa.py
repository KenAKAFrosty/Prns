from __future__ import annotations

import hashlib
import os
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from validation.hardening.embedded_isa.architecture import (
    ArchitectureAdapterError,
    command_for,
)
from validation.hardening.embedded_isa.artifacts import clear, directory as artifact_directory
from validation.hardening.embedded_isa.contract import (
    INVENTORY_PATH,
    ROOT,
    InventoryError,
    load_inventory,
)
from validation.hardening.embedded_isa.error import EmbeddedIsaError
from validation.hardening.embedded_isa.run import require_emulator_identity
from validation.hardening.embedded_isa.transcript import concise, parse, require_match


class EmbeddedIsaTests(unittest.TestCase):
    def test_inventory_defines_the_arm_runner_and_shared_kernel(self) -> None:
        inventory = load_inventory()

        self.assertEqual(inventory.kernel.scenario, "shared-state-machines")
        self.assertEqual(inventory.kernel.completed_scenarios, 2)
        self.assertEqual(inventory.rust_toolchain, "1.96.0")
        self.assertEqual(
            [architecture.identifier for architecture in inventory.architectures],
            ["thumbv7em"],
        )
        arm = inventory.architecture_for_suite("embedded-isa-thumbv7em")
        self.assertEqual(arm.rust_target, "thumbv7em-none-eabihf")
        self.assertEqual(arm.emulator.version, "11.1.1")
        self.assertEqual(len(arm.emulator.source_sha256), 64)
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


if __name__ == "__main__":
    unittest.main()
