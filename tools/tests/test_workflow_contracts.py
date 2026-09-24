from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


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


class ReleaseSdkBootstrapTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/release-readiness.yml").read_text()

    def test_current_release_workflow_bootstraps_sdk(self) -> None:
        self.assertEqual(contracts.validate_release_sdk_bootstrap(self.workflow), [])

    def test_rejects_nonexistent_sdk_matrix_group(self) -> None:
        workflow = self.workflow.replace(
            "matrix.id == 'react-native-sdk' || matrix.id == 'react-native-generated'",
            "matrix.group == 'react-native-sdk'",
        )
        self.assertEqual(
            contracts.validate_release_sdk_bootstrap(workflow),
            ["release-readiness.yml must bootstrap both standalone SDK suite IDs"],
        )

    def test_rejects_missing_or_late_core_contract_build(self) -> None:
        command = "          npm --prefix prns-js run build:code\n"
        for case, workflow in (
            ("missing", self.workflow.replace(command, "")),
            ("after SDK install", self.workflow.replace(command, "").replace(
                "      - uses: astral-sh/setup-uv@", command + "      - uses: astral-sh/setup-uv@"
            )),
        ):
            with self.subTest(case=case):
                self.assertIn(
                    "release-readiness.yml must select the pinned SDK toolchain, build the core contract, "
                    "then install the standalone SDK dependencies",
                    contracts.validate_release_sdk_bootstrap(workflow),
                )

    def test_rejects_installing_rust_without_selecting_it(self) -> None:
        workflow = self.workflow.replace(
            "          printf 'RUSTUP_TOOLCHAIN=1.98.1\\n' >> \"$GITHUB_ENV\"\n", ""
        )
        self.assertIn(
            "release-readiness.yml must select the pinned SDK toolchain, build the core contract, "
            "then install the standalone SDK dependencies",
            contracts.validate_release_sdk_bootstrap(workflow),
        )


class ReleaseApplicationBootstrapTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/release-readiness.yml").read_text()

    def test_current_release_workflow_bootstraps_app_suites(self) -> None:
        self.assertEqual(contracts.validate_release_application_bootstrap(self.workflow), [])

    def test_rejects_setup_that_does_not_select_the_app_suite(self) -> None:
        for suite in ("application-lxmf-direct", "application-detached-mobility"):
            with self.subTest(suite=suite):
                workflow = self.workflow.replace(f"if: matrix.id == '{suite}'", "if: false")
                self.assertIn(
                    f"release-readiness.yml must prepare the required toolchain for {suite}",
                    contracts.validate_release_application_bootstrap(workflow),
                )

    def test_rejects_missing_targets_or_targets_on_the_wrong_toolchain(self) -> None:
        for before, after, suite in (
            ('--toolchain "$RUSTUP_TOOLCHAIN" thumbv7em-none-eabihf',
             '--toolchain stable thumbv7em-none-eabihf', "application-lxmf-direct"),
            ('--toolchain "$RUSTUP_TOOLCHAIN" thumbv7em-none-eabihf',
             '--toolchain "$RUSTUP_TOOLCHAIN"', "application-lxmf-direct"),
            ("rustup toolchain install stable", "rustup toolchain install 1.96.0",
             "application-detached-mobility"),
            (" --target thumbv7em-none-eabihf", "", "application-detached-mobility"),
            (" --component clippy", "", "application-detached-mobility"),
            ("npm install --global npm@11.16.0", "npm install --global npm@latest",
             "application-detached-mobility"),
        ):
            with self.subTest(before=before, after=after):
                self.assertIn(
                    f"release-readiness.yml must prepare the required toolchain for {suite}",
                    contracts.validate_release_application_bootstrap(
                        self.workflow.replace(before, after)
                    ),
                )

    def test_rejects_setup_after_suite_execution(self) -> None:
        start = self.workflow.index("      - name: Prepare application LXMF target\n")
        end = self.workflow.index("      - name: Prepare Linux native development packages\n")
        setup = self.workflow[start:end]
        workflow = self.workflow[:start] + self.workflow[end:]
        workflow = workflow.replace("      - if: always()\n", setup + "      - if: always()\n", 1)
        for suite in ("application-lxmf-direct", "application-detached-mobility"):
            with self.subTest(suite=suite):
                self.assertIn(
                    f"release-readiness.yml must prepare {suite} before running its suite",
                    contracts.validate_release_application_bootstrap(workflow),
                )


class ApplicationMobileCiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text()

    def test_current_mobile_builds_are_gated(self) -> None:
        self.assertEqual(contracts.validate_application_mobile_ci(self.workflow), [])

    def test_rejects_mobile_builds_without_release_gate(self) -> None:
        workflow = self.workflow.replace("      - application-mobile-build\n", "")
        self.assertIn(
            "release-critical must require both app mobile builds",
            contracts.validate_application_mobile_ci(workflow),
        )

    def test_rejects_installing_app_before_its_staged_sdk_exists(self) -> None:
        workflow = self.workflow.replace(
            "          python3 applications/tools/generated-bindings/generate.py stage\n", ""
        )
        self.assertIn(
            "app mobile CI must stage its SDK before installing and compiling the app",
            contracts.validate_application_mobile_ci(workflow),
        )


if __name__ == "__main__":
    unittest.main()
