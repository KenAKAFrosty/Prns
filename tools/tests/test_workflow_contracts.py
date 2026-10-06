from __future__ import annotations

import gzip
import importlib.util
import json
import os
import re
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


class WorkflowSchedulingTests(unittest.TestCase):
    def workflow_jobs(self, name: str) -> dict[str, str]:
        return dict(contracts.workflow_jobs(
            (ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8")
        ))

    def needs(self, block: str) -> set[str]:
        match = re.search(r"(?m)^    needs: (.+)$", block)
        self.assertIsNotNone(match, "job must declare its prerequisites")
        return {job.strip() for job in match.group(1).strip("[]").split(",")}

    def test_only_emulated_suites_wait_for_emulator_preparation(self) -> None:
        for workflow, lane, output, anchor, selector in (
            ("release-readiness.yml", "qualify", "emulated", "qualification-steps",
             "--tier release"),
            ("deep-validation.yml", "hardening", "hardening_emulated", "hardening-steps",
             '--domain hardening --tier "$VALIDATION_TIER"'),
        ):
            with self.subTest(workflow=workflow):
                jobs = self.workflow_jobs(workflow)
                emulated = f"{lane}-emulated"
                self.assertEqual(self.needs(jobs[lane]), {"inventory"})
                self.assertEqual(self.needs(jobs[emulated]), {"inventory", "embedded-emulators"})
                self.assertIn(f"steps: &{anchor}\n", jobs[lane])
                self.assertIn(f"steps: *{anchor}\n", jobs[emulated])
                self.assertIn(f"fromJSON(needs.inventory.outputs.{output})", jobs[emulated])
                self.assertIn(f"{selector} --emulators none)", jobs["inventory"])
                self.assertIn(f"{selector} --emulators required)", jobs["inventory"])
                self.assertTrue({lane, emulated} <= self.needs(jobs["embedded-assurance"]))
                gate = "aggregate" if lane == "qualify" else "deep-validation"
                self.assertTrue({lane, emulated, "embedded-assurance"} <= self.needs(jobs[gate]))
                self.assertIn("    if: always()\n", jobs[gate])
                if lane == "qualify":
                    self.assertIn(
                        'validation/run.py aggregate --tier release --expected-sha "${{ github.sha }}"',
                        jobs[gate],
                    )

    def test_release_setup_keeps_direct_and_nested_tool_consumers(self) -> None:
        qualify = self.workflow_jobs("release-readiness.yml")["qualify"]
        steps = re.split(r"(?m)(?=^      - )", qualify)
        selectors = {
            "node": "uses: actions/setup-node@",
            "uv": "uses: astral-sh/setup-uv@",
            "llvm": "name: Prepare embedded object inspection tools",
            "native": "name: Prepare Linux native development packages",
        }
        conditions = {}
        for tool, selector in selectors.items():
            step = next(step for step in steps if selector in step)
            conditions[tool] = re.search(r"(?m)^        if: (.+)$", step).group(1)

        selected = {tool: set() for tool in selectors}
        matrix = subprocess.run(
            [sys.executable, str(ROOT / "validation/run.py"), "matrix", "--tier", "release"],
            check=True, capture_output=True, text=True,
        )
        suites = json.loads(matrix.stdout)["include"]
        for suite in suites:
            for tool, condition in conditions.items():
                expression = re.sub(
                    r"matrix\.(\w+)", lambda match: repr(suite.get(match.group(1), "")),
                    condition,
                )
                os_name = "Linux" if suite["runner"].startswith("ubuntu-") else "Other"
                expression = expression.replace("runner.os", repr(os_name))
                expression = expression.replace("&&", "and").replace("||", "or")
                if eval(expression, {"__builtins__": {}}):
                    selected[tool].add(suite["id"])

        # Include indirect npm callers, not only suites whose command starts with npm.
        self.assertEqual(selected["node"], {
            "hopspot-javascript-package", "javascript-browser-package", "javascript-contract",
            "wasm-auto-wifi", "wasm-casework", "wasm-events", "wasm-websocket",
            "flasher-web", "esp32-firmware-check", "shipping-firmware",
            "dependency-audit", "release-contracts",
        })
        self.assertEqual(selected["uv"], {
            suite["id"] for suite in suites if suite["domain"] in {"oracles", "interop"}
        } | {"release-contracts"})
        self.assertEqual(selected["llvm"], {
            "embedded-builds", "esp32-firmware-check", "shipping-firmware",
            "embedded-isa-riscv32imac", "embedded-isa-thumbv7em", "embedded-isa-xtensa-esp32s3",
            "embedded-platform-esp32s3", "embedded-platform-nrf52840",
        })
        self.assertTrue({
            "host-workspaces", "integration-capstones", "sanitizer-address",
            "sanitizer-leak", "sanitizer-thread", "embedded-platform-nrf52840",
        } <= selected["native"])
        for suite in suites:
            if suite["domain"] in {"kani", "fuzz", "oracles"}:
                self.assertNotIn(suite["id"], selected["native"])

    def test_embedded_assurance_collects_complete_miri_before_summarizing(self) -> None:
        for workflow in ("deep-validation.yml", "release-readiness.yml"):
            with self.subTest(workflow=workflow):
                job = self.workflow_jobs(workflow)["embedded-assurance"]
                self.assertLess(
                    job.index("validation/run.py aggregate"),
                    job.index("./tools/prns build embedded assurance summarize"),
                )
                expected = "embedded-miri-full" if workflow == "deep-validation.yml" else "embedded-miri-quick"
                self.assertIn(f"--suite {expected}", job)
                self.assertIn("--proofs validation-artifacts/results", job)
                self.assertIn("dtolnay/rust-toolchain@", job)

    def test_exhaustive_miri_has_an_independent_weekly_and_manual_workflow(self) -> None:
        workflow = (ROOT / ".github/workflows/embedded-miri-deep.yml").read_text()
        self.assertIn('cron: "17 9 * * 1"', workflow)
        self.assertIn('workflow_dispatch:', workflow)
        jobs = self.workflow_jobs("embedded-miri-deep.yml")
        self.assertIn('--tier scheduled --suite embedded-miri-full', jobs["inventory"])
        self.assertIn('--tier scheduled --suite embedded-miri-full', jobs["collect"])
        self.assertIn('if: always()', jobs["collect"])
        release = (ROOT / ".github/workflows/release-readiness.yml").read_text()
        self.assertNotIn('embedded-miri-full', release)
        self.assertNotIn('continue-on-error', workflow)

    def test_deep_validation_rejects_every_unsuccessful_lane(self) -> None:
        aggregate = self.workflow_jobs("deep-validation.yml")["deep-validation"]
        bindings = dict(re.findall(
            r"(?m)^      (\w+): \$\{\{ needs\.([\w-]+)\.result \}\}$", aggregate
        ))
        self.assertEqual(set(bindings.values()), self.needs(aggregate))
        script = textwrap.dedent(aggregate.split("      - run: |\n", 1)[1])
        successful = dict.fromkeys(bindings, "success")
        scenarios = [("all successful", successful, 0)]
        for variable in bindings:
            for status in ("failure", "cancelled", "skipped"):
                scenarios.append((f"{variable}={status}", {**successful, variable: status}, 1))
        for name, results, expected in scenarios:
            with self.subTest(name=name):
                result = subprocess.run(
                    ["bash", "-e", "-o", "pipefail", "-c", script],
                    env={**os.environ, **results}, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_feature_aggregate_rejects_every_unsuccessful_lane(self) -> None:
        jobs = self.workflow_jobs("ci.yml")
        aggregate = jobs["feature-configs"]
        lanes = {"feature-core", "feature-tokio", "feature-embassy", "feature-applications"}
        self.assertEqual(self.needs(aggregate), lanes)
        self.assertIn("    if: always()\n", aggregate)
        self.assertTrue(lanes <= jobs.keys())
        for lane in lanes:
            self.assertNotRegex(jobs[lane], r"(?m)^    (?:needs|if):")
        bindings = dict(re.findall(
            r"(?m)^      (\w+): \$\{\{ needs\.([\w-]+)\.result \}\}$", aggregate
        ))
        self.assertEqual(set(bindings.values()), lanes)
        script = textwrap.dedent(aggregate.split("        run: |\n", 1)[1])
        successful = dict.fromkeys(bindings, "success")
        scenarios = [("all successful", successful, 0)]
        for variable in bindings:
            for status in ("failure", "cancelled", "skipped"):
                scenarios.append((f"{variable}={status}", {**successful, variable: status}, 1))
        for name, results, expected in scenarios:
            with self.subTest(name=name):
                result = subprocess.run(
                    ["bash", "-e", "-o", "pipefail", "-c", script],
                    env={**os.environ, **results}, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, expected, result.stderr)
        self.assertIn("- feature-configs\n", jobs["release-critical"])

    def test_sdk_builds_overlap_contracts_but_publication_still_requires_them(self) -> None:
        jobs = self.workflow_jobs("host-sdks.yml")
        self.assertIn("preflight", jobs)
        for job in ("contract", "native", "android", "rust-packages"):
            with self.subTest(job=job):
                self.assertEqual(self.needs(jobs[job]), {"preflight"})
        publishers = {name for name in jobs if name.startswith("publish-")}
        self.assertEqual(publishers, {
            "publish-python", "publish-dotnet", "publish-maven", "publish-crates",
        })
        for name in publishers:
            with self.subTest(job=name):
                self.assertIn("contract", self.needs(jobs[name]))
                condition = jobs[name].split("    needs:", 1)[0]
                # Without status overrides, GitHub requires successful prerequisites.
                self.assertNotRegex(condition, r"\b(?:always|failure|cancelled)\s*\(")

    def test_javascript_browser_runs_independently_but_both_lanes_gate_release(self) -> None:
        jobs = self.workflow_jobs("napi.yml")
        self.assertNotRegex(jobs["javascript-browser"], r"(?m)^    (?:needs|if):")
        self.assertNotIn("actions/download-artifact@", jobs["javascript-browser"])
        self.assertEqual(self.needs(jobs["javascript-native"]), {"napi-build"})
        self.assertIn("bindings-x86_64-unknown-linux-gnu", jobs["javascript-native"])
        aggregate = jobs["javascript-hosts"]
        self.assertEqual(self.needs(aggregate), {"javascript-browser", "javascript-native"})
        self.assertIn("    if: always()\n", aggregate)
        self.assertIn("BROWSER_RESULT: ${{ needs.javascript-browser.result }}", aggregate)
        self.assertIn("NATIVE_RESULT: ${{ needs.javascript-native.result }}", aggregate)
        script = textwrap.dedent(aggregate.split("        run: |\n", 1)[1])
        for browser in ("success", "failure", "cancelled", "skipped"):
            for native in ("success", "failure", "cancelled", "skipped"):
                with self.subTest(browser=browser, native=native):
                    result = subprocess.run(
                        ["bash", "-e", "-o", "pipefail", "-c", script],
                        env={**os.environ, "BROWSER_RESULT": browser, "NATIVE_RESULT": native},
                        capture_output=True, text=True,
                    )
                    expected = 0 if browser == native == "success" else 1
                    self.assertEqual(result.returncode, expected, result.stderr)
        self.assertIn("- javascript-hosts\n", jobs["napi-release-critical"])
        self.assertIn("napi-release-critical", self.needs(jobs["npm-stage"]))
        self.assertEqual(self.needs(jobs["napi-publish"]), {"npm-stage"})


if __name__ == "__main__":
    unittest.main()
