from __future__ import annotations

import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from validation.hardening.embedded_isa.contract import (
    Compiler,
    load_inventory,
)
from validation.hardening.embedded_host import HostPlatform
from validation.hardening.embedded_platform.contract import (
    load_inventory as load_platform_inventory,
)
from validation.hardening.embedded_readiness import (
    CheckState,
    CommandOutput,
    EspEnvironment,
    ReadinessCheck,
    ReadinessContract,
    ReadinessError,
    ReadinessLane,
    ReadinessStatus,
    assignments,
    inspect,
    load_esp_environment,
    load_esp_identity,
)
from validation.hardening.embedded_readiness.run import render


class FakeProbe:
    def __init__(
        self,
        paths: dict[str, Path],
        outputs: dict[tuple[str, ...], CommandOutput],
        fingerprints: dict[Path, str] | None = None,
        environment: dict[str, str] | None = None,
    ) -> None:
        self.paths = paths
        self.outputs = outputs
        self.fingerprints = fingerprints or {}
        self.environment_values = environment or {}

    def environment(self, name: str) -> str | None:
        return self.environment_values.get(name)

    def find(self, command: str, search_paths: tuple[Path, ...] = ()) -> Path | None:
        return self.paths.get(command)

    def host_platform(self) -> HostPlatform:
        return HostPlatform.MACOS_ARM64

    def run(self, command: tuple[str, ...]) -> CommandOutput:
        return self.outputs.get(command, CommandOutput(127, "", "missing command"))

    def fingerprint(self, path: Path) -> str | None:
        return self.fingerprints.get(path)


class EmbeddedReadinessTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        libclang = root / "libclang"
        libclang.mkdir()
        (libclang / "libclang.dylib").write_bytes(b"library")
        inventory = load_inventory()
        platform_inventory = load_platform_inventory()
        self.contract = ReadinessContract(
            isa_toolchain=inventory.rust_toolchain,
            architectures=inventory.architectures,
            platforms=platform_inventory.platforms,
            miri_toolchain="nightly-2025-11-21",
            miri_scenarios=3,
            esp_identity=load_esp_identity(),
            esp_environment=EspEnvironment((root / "bin",), libclang),
        )
        self.paths = {
            "espup": root / "espup",
            "xtensa-esp32s3-elf-gcc": root / "xtensa-esp32s3-elf-gcc",
            "xtensa-esp32s3-elf-objdump": root / "xtensa-esp32s3-elf-objdump",
            "qemu-system-arm": root / "qemu-system-arm",
            "qemu-system-riscv32": root / "qemu-system-riscv32",
            "qemu-system-xtensa": root / "qemu-system-xtensa",
            "renode": root / "Renode" / "renode",
        }
        platform = self.contract.platforms[0]
        self.platform_description = (
            self.paths["renode"].parent / platform.platform_description
        )
        self.fingerprints = {
            self.platform_description: platform.platform_description_sha256
        }
        targets = "\n".join(
            architecture.rust_target for architecture in inventory.architectures
        )
        identity = self.contract.esp_identity
        self.outputs = {
            ("rustup", "run", "1.96.0", "rustc", "--version"): CommandOutput(
                0, "rustc 1.96.0 (commit)\n", ""
            ),
            (
                "rustup",
                "target",
                "list",
                "--toolchain",
                "1.96.0",
                "--installed",
            ): CommandOutput(0, targets, ""),
            (
                "rustup",
                "component",
                "list",
                "--toolchain",
                "1.96.0",
                "--installed",
            ): CommandOutput(0, "", ""),
            ("rustup", "run", "stable", "rustc", "--version"): CommandOutput(
                0, "rustc 1.98.0 (commit)\n", ""
            ),
            (
                "rustup",
                "target",
                "list",
                "--toolchain",
                "stable",
                "--installed",
            ): CommandOutput(0, targets, ""),
            (
                "rustup",
                "component",
                "list",
                "--toolchain",
                "stable",
                "--installed",
            ): CommandOutput(0, "llvm-tools-test-host\n", ""),
            (
                "rustup",
                "run",
                "nightly-2025-11-21",
                "rustc",
                "--version",
            ): CommandOutput(0, "rustc 1.93.0-nightly (commit)\n", ""),
            (
                "rustup",
                "component",
                "list",
                "--toolchain",
                "nightly-2025-11-21",
                "--installed",
            ): CommandOutput(0, "miri-test-host\nrust-src\n", ""),
            (
                "rustup",
                "run",
                "nightly-2025-11-21",
                "cargo",
                "miri",
                "--version",
            ): CommandOutput(0, "miri 0.1.0\n", ""),
            (str(self.paths["espup"]), "--version"): CommandOutput(
                0, f"espup {identity.espup_version}\n", ""
            ),
            ("rustup", "run", "esp", "rustc", "-vV"): CommandOutput(
                0, f"{identity.rustc_banner}\nrelease: ignored\n", ""
            ),
            (str(self.paths["xtensa-esp32s3-elf-gcc"]), "--version"): CommandOutput(
                0, f"{identity.gcc_banner}\n", ""
            ),
            (str(self.paths["xtensa-esp32s3-elf-objdump"]), "--version"): CommandOutput(
                0, f"{identity.objdump_banner}\n", ""
            ),
            (str(self.paths["qemu-system-arm"]), "--version"): CommandOutput(
                0, "QEMU emulator version 11.1.1\n", ""
            ),
            (str(self.paths["qemu-system-riscv32"]), "--version"): CommandOutput(
                0, "QEMU emulator version 11.1.1\n", ""
            ),
            (str(self.paths["qemu-system-xtensa"]), "--version"): CommandOutput(
                0,
                "QEMU emulator version 9.2.2 (esp_develop_9.2.2_20260417)\n",
                "",
            ),
            (str(self.paths["renode"]), "--version"): CommandOutput(
                0, "\n".join(platform.emulator.identity) + "\n", ""
            ),
        }

    def test_ready_environment_satisfies_every_derived_requirement(self) -> None:
        checks = inspect(
            self.contract,
            FakeProbe(self.paths, self.outputs, self.fingerprints),
        )

        self.assertEqual(len(checks), 8)
        self.assertTrue(all(check.state is CheckState.READY for check in checks))
        self.assertEqual(
            {check.subject for check in checks},
            {
                "target-ISA Rust",
                "resource Rust",
                "embedded Miri",
                "ESP resource toolchain",
                "thumbv7em emulator",
                "riscv32imac emulator",
                "xtensa-esp32s3 emulator",
                "nrf52840 platform emulator",
            },
        )

    def test_platform_pilot_checks_exact_emulator_and_model_identities(self) -> None:
        checks = inspect(
            self.contract,
            FakeProbe(self.paths, self.outputs, self.fingerprints),
        )

        pilot = next(check for check in checks if check.lane is ReadinessLane.PILOT)

        self.assertEqual(pilot.state, CheckState.READY)
        self.assertIn(str(self.platform_description), pilot.detail)

    def test_platform_pilot_rejects_a_modified_model(self) -> None:
        fingerprints = {self.platform_description: "0" * 64}

        checks = inspect(
            self.contract,
            FakeProbe(self.paths, self.outputs, fingerprints),
        )
        pilot = next(check for check in checks if check.lane is ReadinessLane.PILOT)

        self.assertEqual(pilot.state, CheckState.MISMATCH)
        self.assertIn("model checksum mismatch", pilot.detail)

    def test_platform_pilot_setup_uses_the_host_package(self) -> None:
        paths = dict(self.paths)
        del paths["renode"]

        checks = inspect(self.contract, FakeProbe(paths, self.outputs))
        pilot = next(check for check in checks if check.lane is ReadinessLane.PILOT)

        self.assertEqual(pilot.state, CheckState.MISSING)
        guidance = "\n".join(pilot.setup)
        self.assertIn("renode-1.17.0.osx-arm64-portable.dmg", guidance)
        self.assertIn(
            "63b1fb691207f503cea937e4ec8fad3e068a517d0abac3c24c0206c62f6c4d12",
            guidance,
        )

    def test_esp_emulator_setup_selects_the_current_host_package(self) -> None:
        paths = dict(self.paths)
        del paths["qemu-system-xtensa"]

        checks = inspect(
            self.contract, FakeProbe(paths, self.outputs, self.fingerprints)
        )
        xtensa = next(
            check for check in checks if check.subject == "xtensa-esp32s3 emulator"
        )

        self.assertEqual(xtensa.state, CheckState.MISSING)
        guidance = "\n".join(xtensa.setup)
        self.assertIn("aarch64-apple-darwin.tar.xz", guidance)
        self.assertIn(
            "bb8c15810565d3df1665dc34962430885e11bc95575b228fb44698146be1e9d6",
            guidance,
        )
        self.assertIn("brew install libgcrypt glib pixman sdl2 libslirp", guidance)

    def test_aggregate_status_preserves_any_failed_check(self) -> None:
        ready = ReadinessCheck(
            ReadinessLane.MIRI, "ready", CheckState.READY, "available"
        )
        missing = ReadinessCheck(
            ReadinessLane.ISA, "missing", CheckState.MISSING, "unavailable"
        )

        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            status = render((missing, ready))

        self.assertEqual(status, ReadinessStatus.NOT_READY)

    def test_missing_target_and_emulator_produce_exact_setup_guidance(self) -> None:
        outputs = dict(self.outputs)
        target_command = (
            "rustup",
            "target",
            "list",
            "--toolchain",
            "1.96.0",
            "--installed",
        )
        outputs[target_command] = CommandOutput(
            0, "thumbv7em-none-eabihf\n", ""
        )
        paths = dict(self.paths)
        del paths["qemu-system-riscv32"]

        checks = inspect(self.contract, FakeProbe(paths, outputs, self.fingerprints))
        rust = next(check for check in checks if check.subject == "target-ISA Rust")
        emulator = next(
            check for check in checks if check.subject == "riscv32imac emulator"
        )

        self.assertEqual(rust.state, CheckState.MISSING)
        self.assertIn("riscv32imac-unknown-none-elf", rust.detail)
        self.assertIn(
            "rustup target add --toolchain 1.96.0",
            "\n".join(rust.setup),
        )
        self.assertEqual(emulator.state, CheckState.MISSING)
        self.assertIn(
            "https://download.qemu.org/qemu-11.1.1.tar.xz",
            "\n".join(emulator.setup),
        )

    def test_wrong_emulator_version_is_not_treated_as_ready(self) -> None:
        outputs = dict(self.outputs)
        outputs[(str(self.paths["qemu-system-arm"]), "--version")] = CommandOutput(
            0, "QEMU emulator version 11.1.2\n", ""
        )

        checks = inspect(
            self.contract, FakeProbe(self.paths, outputs, self.fingerprints)
        )
        arm = next(check for check in checks if check.subject == "thumbv7em emulator")

        self.assertEqual(arm.state, CheckState.MISMATCH)
        self.assertIn("11.1.1", arm.detail)

    def test_esp_check_reports_missing_and_mismatched_tools_together(self) -> None:
        paths = dict(self.paths)
        del paths["espup"]
        outputs = dict(self.outputs)
        outputs[(str(self.paths["xtensa-esp32s3-elf-gcc"]), "--version")] = (
            CommandOutput(0, "xtensa-esp32s3-elf-gcc (wrong)\n", "")
        )

        checks = inspect(self.contract, FakeProbe(paths, outputs, self.fingerprints))
        esp = next(
            check for check in checks if check.subject == "ESP resource toolchain"
        )

        self.assertEqual(esp.state, CheckState.MISMATCH)
        self.assertIn("gcc=", esp.detail)
        self.assertIn("missing espup", esp.detail)

    def test_esp_compiler_targets_are_not_requested_from_upstream_rust(self) -> None:
        architectures = (
            self.contract.architectures[0],
            replace(self.contract.architectures[1], compiler=Compiler.ESP),
        )
        contract = replace(self.contract, architectures=architectures)
        outputs = dict(self.outputs)
        for toolchain in ("1.96.0", "stable"):
            outputs[
                (
                    "rustup",
                    "target",
                    "list",
                    "--toolchain",
                    toolchain,
                    "--installed",
                )
            ] = CommandOutput(0, "thumbv7em-none-eabihf\n", "")

        checks = inspect(contract, FakeProbe(self.paths, outputs, self.fingerprints))

        upstream = {
            check.subject: check
            for check in checks
            if check.subject in {"target-ISA Rust", "resource Rust"}
        }
        self.assertTrue(all(check.passed() for check in upstream.values()))

    def test_empty_libclang_directory_is_not_ready(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            contract = replace(
                self.contract,
                esp_environment=replace(
                    self.contract.esp_environment,
                    libclang_path=Path(directory),
                ),
            )

            checks = inspect(
                contract, FakeProbe(self.paths, self.outputs, self.fingerprints)
            )
            esp = next(
                check for check in checks if check.subject == "ESP resource toolchain"
            )

            self.assertEqual(esp.state, CheckState.MISSING)
            self.assertIn("LIBCLANG_PATH", esp.detail)

    def test_objdump_identity_and_toolchain_directory_are_enforced(self) -> None:
        paths = dict(self.paths)
        paths["xtensa-esp32s3-elf-objdump"] = (
            self.paths["xtensa-esp32s3-elf-objdump"].parent
            / "different"
            / "xtensa-esp32s3-elf-objdump"
        )
        outputs = dict(self.outputs)
        outputs[(str(paths["xtensa-esp32s3-elf-objdump"]), "--version")] = (
            CommandOutput(0, "GNU objdump (wrong) 2.45\n", "")
        )

        checks = inspect(self.contract, FakeProbe(paths, outputs, self.fingerprints))
        esp = next(
            check for check in checks if check.subject == "ESP resource toolchain"
        )

        self.assertEqual(esp.state, CheckState.MISMATCH)
        self.assertIn("objdump=", esp.detail)
        self.assertIn("different toolchain directories", esp.detail)

    def test_esp_identity_requires_every_canonical_field(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            identity = Path(directory) / "identity.sh"
            identity.write_text('ESPUP_VERSION="0.17.1"\n', encoding="utf-8")

            with self.assertRaises(ReadinessError):
                load_esp_identity(identity)

    def test_esp_export_paths_are_parsed_without_executing_shell(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            libclang = home / "clang" / "lib"
            libclang.mkdir(parents=True)
            (home / "export-esp.sh").write_text(
                'export PATH="$HOME/toolchain/bin:$PATH"\n'
                'export LIBCLANG_PATH="$HOME/clang/lib"\n',
                encoding="utf-8",
            )

            environment = load_esp_environment(home)

            self.assertIn(home / "toolchain" / "bin", environment.search_paths)
            self.assertEqual(environment.libclang_path, libclang)

    def test_inherited_libclang_path_is_used_without_an_export_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            libclang = home / "inherited" / "lib"
            libclang.mkdir(parents=True)

            with patch.dict("os.environ", {"LIBCLANG_PATH": str(libclang)}):
                environment = load_esp_environment(home)

            self.assertEqual(environment.libclang_path, libclang)

    def test_assignment_parser_rejects_unbalanced_quotes(self) -> None:
        with self.assertRaises(ReadinessError):
            assignments('VALUE="unfinished')


if __name__ == "__main__":
    unittest.main()
