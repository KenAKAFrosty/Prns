from __future__ import annotations

import importlib.util
import gzip
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "validation" / "release" / "workflow-contracts.py"
SPEC = importlib.util.spec_from_file_location("workflow_contracts", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
contracts = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = contracts
SPEC.loader.exec_module(contracts)


class WorkflowCompilerEnvironmentTests(unittest.TestCase):
    def test_rejects_workflow_global_rustflags(self) -> None:
        workflow = """name: ci
env:
  RUSTFLAGS: \"-D warnings --cfg aes_armv8\"
jobs:
  embedded:
    runs-on: ubuntu-latest
    steps:
      - run: cargo check --target thumbv7em-none-eabihf
"""

        self.assertEqual(
            contracts.validate_ci_compiler_environment(workflow),
            [
                "ci.yml must not define workflow-global RUSTFLAGS; cross-toolchain jobs "
                "inherit them"
            ],
        )

    def test_rejects_host_rustflags_on_a_cross_toolchain_job(self) -> None:
        workflow = """name: ci
env:
  CARGO_TERM_COLOR: always
jobs:
  embedded:
    runs-on: ubuntu-latest
    env:
      RUSTFLAGS: \"-D warnings --cfg aes_armv8\"
    steps:
      - run: cargo check --target riscv32imac-unknown-none-elf
"""

        self.assertEqual(
            contracts.validate_ci_compiler_environment(workflow),
            [
                "ci.yml cross-toolchain job embedded must not define host RUSTFLAGS"
            ],
        )

    def test_allows_job_scoped_rustflags_for_host_only_work(self) -> None:
        workflow = """name: ci
env:
  CARGO_TERM_COLOR: always
jobs:
  host:
    runs-on: ubuntu-latest
    env:
      RUSTFLAGS: \"-D warnings --cfg aes_armv8\"
    steps:
      - run: cargo test --workspace --locked
"""

        self.assertEqual(contracts.validate_ci_compiler_environment(workflow), [])


class SwiftSetupTests(unittest.TestCase):
    def test_composite_action_dependencies_require_locked_revisions(self) -> None:
        definition = ROOT / ".github/actions/setup-swift/action.yml"
        read_text = Path.read_text

        def read(path: Path, *args: object, **kwargs: object) -> str:
            text = read_text(path, *args, **kwargs)
            if path == definition:
                return text.replace("364295d9c23900ce04d4e5cc708387921b4e50f9", "main")
            return text

        with mock.patch.object(Path, "read_text", read):
            self.assertEqual(contracts.validate(), [
                ".github/actions/setup-swift/action.yml: swift-actions/setup-swift@main "
                "must use 364295d9c23900ce04d4e5cc708387921b4e50f9"
            ])

    def test_signing_key_import_fails_closed(self) -> None:
        action = (ROOT / ".github/actions/setup-swift/action.yml").read_text()
        script = textwrap.dedent(action.split("      run: |\n", 1)[1].split("    - uses:", 1)[0])
        fingerprint = "E813C892820A6FA13755B268F167DF1ACF9CE069"
        cases = (
            (fingerprint, "0", "0", False, 0, "imported\n"),
            (fingerprint, "0", "0", True, 0, "imported\n"),
            ("0" * 40, "0", "0", False, 1, ""),
            ("0" * 40, "0", "0", True, 1, ""),
            (fingerprint, "1", "0", False, 1, ""),
            (fingerprint, "0", "3", False, 3, "imported\n"),
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            curl = root / "curl"
            curl.write_text(
                '#!/bin/sh\n[ "$SWIFT_TEST_DOWNLOAD_EXIT" = 0 ] || exit "$SWIFT_TEST_DOWNLOAD_EXIT"\n'
                'cp "$SWIFT_TEST_KEY_SOURCE" "$RUNNER_TEMP/swiftly-signing-key.asc"\n'
            )
            curl.chmod(0o700)
            gpg = root / "gpg"
            gpg.write_text(
                '#!/bin/sh\nfor path do :; done\n'
                '[ "$(cat "$path")" = public-key-fixture ] || exit 98\ncase "$*" in\n'
                '*--show-keys*) printf "pub:::::::::\\nfpr:::::::::%s:\\n" "$SWIFT_TEST_FINGERPRINT" ;;\n'
                '*--import*) echo imported; exit "$SWIFT_TEST_IMPORT_EXIT" ;;\n'
                '*) exit 99 ;;\nesac\n'
            )
            gpg.chmod(0o700)
            for key, download_exit, import_exit, compressed, expected_exit, output in cases:
                with self.subTest(key=key, download=download_exit, importing=import_exit, compressed=compressed):
                    payload = b"public-key-fixture\n"
                    source = root / "download"
                    source.write_bytes(gzip.compress(payload) if compressed else payload)
                    result = subprocess.run(
                        ["bash", "-c", script],
                        env={
                            **os.environ,
                            "PATH": f"{root}{os.pathsep}{os.environ['PATH']}",
                            "RUNNER_TEMP": str(root),
                            "SWIFT_TEST_FINGERPRINT": key,
                            "SWIFT_TEST_DOWNLOAD_EXIT": download_exit,
                            "SWIFT_TEST_IMPORT_EXIT": import_exit,
                            "SWIFT_TEST_KEY_SOURCE": str(source),
                        },
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    self.assertEqual((result.returncode, result.stdout, result.stderr), (expected_exit, output, ""))

    def test_swift_workflows_share_signature_verifying_setup(self) -> None:
        for name in ("host-sdks.yml", "host-sdk-public-qualification.yml"):
            workflow = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("uses: ./.github/actions/setup-swift", workflow)
            self.assertNotIn("skip-verify-signature", workflow)
        action = (ROOT / ".github/actions/setup-swift/action.yml").read_text()
        self.assertIn("uses: swift-actions/setup-swift@364295d9c23900ce04d4e5cc708387921b4e50f9", action)
        self.assertNotIn("skip-verify-signature", action)


if __name__ == "__main__":
    unittest.main()
