from __future__ import annotations

import hashlib
import io
import shutil
import tarfile
import tempfile
import unittest
from pathlib import Path

from validation.hardening.embedded_host import HostPlatform
from validation.hardening.embedded_readiness.prepare import (
    Archive,
    EmulatorRequirement,
    HostedArchive,
    IdentityScope,
    PreparationError,
    SourceBuild,
    acquire,
    extract_archive,
    install_hosted,
    install_source,
    requirements_for_suites,
    safe_root,
    verify_identity,
)


class EmbeddedPreparationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_suite_requirements_derive_from_emulator_contracts(self) -> None:
        requirements = requirements_for_suites(
            (
                "embedded-isa-thumbv7em",
                "embedded-isa-riscv32imac",
                "embedded-isa-xtensa-esp32s3",
                "embedded-platform-nrf52840",
                "embedded-platform-esp32s3",
            ),
            HostPlatform.LINUX_AMD64,
        )

        self.assertEqual(
            tuple(requirement.executable for requirement in requirements),
            (
                "qemu-system-arm",
                "qemu-system-riscv32",
                "qemu-system-xtensa",
                "renode",
            ),
        )
        xtensa = requirements[2]
        self.assertIsInstance(xtensa.acquisition, HostedArchive)
        self.assertEqual(
            xtensa.acquisition.archive.source_sha256,
            "0eecb2a34a5586c0e59110f77b9343b7b336e82fdb0e1a30e1dc1bab8a547e35",
        )
        renode = requirements[3]
        self.assertEqual(renode.identity_scope, IdentityScope.ALL_LINES)
        self.assertIsNotNone(renode.model)

    def test_unknown_suite_is_rejected(self) -> None:
        with self.assertRaisesRegex(PreparationError, "unknown embedded assurance suite"):
            requirements_for_suites(("not-a-suite",), HostPlatform.LINUX_AMD64)

    def test_download_requires_the_contract_checksum(self) -> None:
        archive = Archive("https://example.com/tool.tar.xz", "0" * 64)

        with self.assertRaisesRegex(PreparationError, "checksum does not match"):
            acquire(
                self.root,
                archive,
                lambda _url, destination: destination.write_bytes(b"wrong"),
            )

        self.assertEqual(tuple((self.root / "downloads").iterdir()), ())

    def test_archive_member_cannot_escape_destination(self) -> None:
        archive = self.root / "escape.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            member = tarfile.TarInfo("../escaped")
            member.size = 4
            output.addfile(member, io.BytesIO(b"nope"))
        destination = self.root / "destination"
        destination.mkdir()

        with self.assertRaisesRegex(PreparationError, "escapes destination"):
            extract_archive(archive, destination)

        self.assertFalse((self.root / "escaped").exists())

    def test_hosted_archive_is_verified_and_located(self) -> None:
        source = self.root / "fixture.tar.gz"
        with tempfile.TemporaryDirectory() as fixture_directory:
            fixture = Path(fixture_directory)
            executable = fixture / "bundle" / "qemu-system-test"
            executable.parent.mkdir()
            executable.write_text(
                "#!/bin/sh\nprintf '%s\\n' 'QEMU emulator version 1.2.3' 'copyright'\n",
                encoding="utf-8",
            )
            executable.chmod(0o755)
            with tarfile.open(source, "w:gz") as output:
                output.add(executable.parent, arcname="bundle")
        checksum = hashlib.sha256(source.read_bytes()).hexdigest()
        acquisition = HostedArchive(
            Archive("https://example.com/tool.tar.gz", checksum)
        )
        requirement = EmulatorRequirement(
            executable="qemu-system-test",
            identity=("QEMU emulator version 1.2.3",),
            identity_scope=IdentityScope.FIRST_LINE,
            acquisition=acquisition,
            model=None,
        )

        installed = install_hosted(
            self.root,
            acquisition,
            (requirement,),
            lambda _url, destination: shutil.copyfile(source, destination),
        )

        executable = installed[requirement.executable]
        verify_identity(executable, requirement.identity, requirement.identity_scope)
        self.assertTrue(executable.is_file())

    def test_source_archive_builds_all_selected_targets_once(self) -> None:
        source = self.root / "source.tar.xz"
        with tempfile.TemporaryDirectory() as fixture_directory:
            fixture = Path(fixture_directory) / "qemu"
            fixture.mkdir()
            configure = fixture / "configure"
            configure.write_text("#!/bin/sh\n", encoding="utf-8")
            configure.chmod(0o755)
            with tarfile.open(source, "w:xz") as output:
                output.add(fixture, arcname="qemu")
        checksum = hashlib.sha256(source.read_bytes()).hexdigest()
        acquisition = SourceBuild(
            Archive("https://example.com/qemu.tar.xz", checksum)
        )
        requirements = tuple(
            EmulatorRequirement(
                executable=name,
                identity=(f"{name} identity",),
                identity_scope=IdentityScope.FIRST_LINE,
                acquisition=acquisition,
                model=None,
            )
            for name in ("qemu-system-arm", "qemu-system-riscv32")
        )
        commands = []
        install = None

        def run(command: tuple[str, ...], cwd: Path) -> None:
            nonlocal install
            commands.append((command, cwd))
            if command[0].endswith("configure"):
                prefix = next(
                    part for part in command if part.startswith("--prefix=")
                )
                install = Path(prefix.split("=", 1)[1])
                return
            assert install is not None
            bin_directory = install / "bin"
            bin_directory.mkdir(parents=True)
            for requirement in requirements:
                executable = bin_directory / requirement.executable
                executable.write_text("#!/bin/sh\n", encoding="utf-8")
                executable.chmod(0o755)

        installed = install_source(
            self.root,
            acquisition,
            requirements,
            lambda _url, destination: shutil.copyfile(source, destination),
            run,
        )

        self.assertEqual(
            set(installed),
            {requirement.executable for requirement in requirements},
        )
        self.assertEqual(len(commands), 2)
        self.assertIn(
            "--target-list=arm-softmmu,riscv32-softmmu",
            commands[0][0],
        )
        self.assertEqual(commands[1][0], ("ninja", "install"))
        self.assertEqual(commands[0][1], commands[1][1])

    def test_broad_installation_roots_are_rejected(self) -> None:
        for root in (Path("/"), Path.home()):
            with self.subTest(root=root), self.assertRaises(PreparationError):
                safe_root(root)


if __name__ == "__main__":
    unittest.main()
