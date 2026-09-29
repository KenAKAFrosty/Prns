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


if __name__ == "__main__":
    unittest.main()
