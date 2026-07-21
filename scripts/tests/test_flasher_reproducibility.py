from __future__ import annotations

import json
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))

from flasher_build_metadata import (
    EXPECTED_TOOLS,
    EXPECTED_WEB_PACKAGES,
    build_metadata,
    validate_metadata,
)
from flasher_candidate_output import resolve_output
from flasher_reproducibility import validate_report
from flasher_sparse_sizes import MERGED_BASELINES, SPARSE_BASELINES, build_report


VERSION = "0.2.6"
COMMIT = "a" * 40


def tools() -> dict[str, str]:
    return {
        "rustc": "rustc 1.96.0 (fixture)",
        "cargo": "cargo 1.96.0 (fixture)",
        "node": "v24.18.0",
        "npm": "11.0.0",
        "dioxus": "dioxus 0.7.5",
        "cargo_binstall": "cargo-binstall 1.21.0",
        "espup": "espup 0.17.1",
        "esp_rustc": "rustc 1.95.0-nightly (fixture)",
        "xtensa_gcc": "xtensa-esp-elf-gcc (crosstool-NG esp-15.2.0_20250920) 15.2.0",
        "llvm_objcopy": "llvm-objcopy version 20.1.8",
        "python": "Python 3.13.0",
        "git": "git version 2.50.0",
    }


def manifest(
    *,
    heltec_size: int = 1_000_000,
    t_beam_size: int = 1_000_000,
    xiao_size: int = 900_000,
) -> dict:
    return {
        "release": {"version": VERSION, "commit": COMMIT},
        "targets": [
            {
                "board_slug": "heltec-v4",
                "transport": "esp-serial",
                "parts": [{"size": heltec_size}],
            },
            {
                "board_slug": "t-beam-supreme",
                "transport": "esp-serial",
                "parts": [{"size": t_beam_size}],
            },
            {
                "board_slug": "xiao-esp32-c6",
                "transport": "esp-serial",
                "parts": [{"size": xiao_size}],
            },
            {
                "board_slug": "t-echo",
                "transport": "uf2-mass-storage",
                "parts": [{"size": 700_000}],
            },
        ],
    }


def run_python(script: str, *arguments: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPTS / script), *(str(value) for value in arguments)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def candidate(root: Path, *, payload: bytes = b"same bytes") -> None:
    root.mkdir(parents=True)
    (root / "flash-manifest.json").write_text(
        json.dumps(
            {
                "schema": 2,
                "release": {
                    "version": VERSION,
                    "channel": "preview",
                    "commit": COMMIT,
                },
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    artifact = root / "firmware" / "fixture.bin"
    artifact.parent.mkdir(parents=True)
    artifact.write_bytes(payload)


def duplicate_member_archive(path: Path) -> None:
    with tarfile.open(path, "w:gz") as archive:
        for payload in (b"first", b"second"):
            member = tarfile.TarInfo("payload.txt")
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))


class FlasherReproducibilityTests(unittest.TestCase):
    def test_build_metadata_is_source_derived_and_exact_pinned(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / "docs" / "website" / "package.json"
            package.parent.mkdir(parents=True)
            package.write_text(
                json.dumps(
                    {
                        "dependencies": {
                            "esptool-js": "0.6.0",
                            "spark-md5": "3.0.2",
                        },
                        "devDependencies": {"esbuild": "0.28.1"},
                    }
                ),
                encoding="utf-8",
            )
            value = build_metadata(
                commit=COMMIT,
                source_date_epoch=1_774_358_400,
                tools=tools(),
                root=root,
                system="Linux",
                machine="x86_64",
            )
            self.assertEqual(value["expected_tools"], EXPECTED_TOOLS)
            self.assertEqual(value["web_packages"], EXPECTED_WEB_PACKAGES)
            self.assertEqual(value["built_at_utc"], "2026-03-24T13:20:00+00:00")
            validate_metadata(value, commit=COMMIT)

            value["tools"]["node"] = "v24.18.1"
            with self.assertRaisesRegex(ValueError, "exact pins"):
                validate_metadata(value, commit=COMMIT)

    def test_sparse_report_covers_all_boards_and_enforces_sixty_percent(self) -> None:
        report = build_report(manifest())
        self.assertEqual(len(report["targets"]), 4)
        self.assertEqual(report["aggregate_esp"]["gate"], "passed")
        heltec = next(
            target for target in report["targets"] if target["board_slug"] == "heltec-v4"
        )
        self.assertEqual(heltec["gate"], "passed")

        maximum = SPARSE_BASELINES["heltec-v4"] * 40 // 100
        with self.assertRaisesRegex(ValueError, "60% reduction gate failed"):
            build_report(manifest(heltec_size=maximum + 1))

        with self.assertRaisesRegex(ValueError, "aggregate ESP sparse total"):
            build_report(
                manifest(
                    heltec_size=SPARSE_BASELINES["heltec-v4"] * 40 // 100,
                    t_beam_size=SPARSE_BASELINES["t-beam-supreme"] * 40 // 100,
                    xiao_size=MERGED_BASELINES["xiao-esp32-c6"],
                )
            )

    def test_candidate_output_is_absolute_before_directory_changes(self) -> None:
        repository = Path("/workspace/prns")
        resolved = resolve_output(
            repository,
            Path("target/flasher-candidate"),
            cwd=repository,
        )
        self.assertEqual(resolved, repository / "target" / "flasher-candidate")
        with self.assertRaisesRegex(ValueError, "beneath target"):
            resolve_output(
                repository,
                Path("docs/website/public/candidate"),
                cwd=repository,
            )
        external = resolve_output(
            repository,
            Path("/tmp/prns-candidate"),
            cwd=repository,
        )
        self.assertEqual(external, Path("/tmp/prns-candidate").resolve())

        build_script = (SCRIPTS / "build-flasher-candidate.sh").read_text(encoding="utf-8")
        resolution = build_script.index("flasher_candidate_output.py")
        first_directory_change = build_script.index('cd "$root/docs/website"')
        self.assertLess(resolution, first_directory_change)

    def test_esp_toolchain_archive_and_resolved_identity_are_exact(self) -> None:
        installer = (SCRIPTS / "install-release-esp-toolchain.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn('crosstool_version="15.2.0_20250920"', installer)
        self.assertIn(
            'gcc_sha256="e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977"',
            installer,
        )
        self.assertIn("--crosstool-toolchain-version", installer)
        self.assertIn("xtensa-esp-elf-gcc (crosstool-NG esp-", installer)
        self.assertNotIn("xtensa-esp32s3-elf-gcc", installer)

    def test_two_independent_archives_must_match_byte_for_byte(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            primary = workspace / "primary"
            reproduction = workspace / "reproduction"
            candidate(primary)
            candidate(reproduction)
            primary_archive = workspace / "primary.tar.gz"
            reproduction_archive = workspace / "reproduction.tar.gz"
            for source, output in (
                (primary, primary_archive),
                (reproduction, reproduction_archive),
            ):
                result = run_python("package-flasher-candidate.py", source, output)
                self.assertEqual(result.returncode, 0, result.stderr)
            output = workspace / "unsigned.tar.gz"
            report = workspace / "reproducibility.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPTS / "compare-flasher-candidates.py"),
                    "--primary",
                    str(primary_archive),
                    "--reproduction",
                    str(reproduction_archive),
                    "--output",
                    str(output),
                    "--report",
                    str(report),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            extracted = workspace / "extracted"
            with tarfile.open(output, "r:gz") as archive:
                archive.extractall(extracted, filter="data")
            validate_report(extracted, version=VERSION, source_commit=COMMIT)

    def test_reproducibility_rejects_payload_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            primary = workspace / "primary"
            reproduction = workspace / "reproduction"
            candidate(primary)
            candidate(reproduction, payload=b"changed bytes")
            primary_archive = workspace / "primary.tar.gz"
            reproduction_archive = workspace / "reproduction.tar.gz"
            for source, output in (
                (primary, primary_archive),
                (reproduction, reproduction_archive),
            ):
                self.assertEqual(
                    run_python("package-flasher-candidate.py", source, output).returncode,
                    0,
                )
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPTS / "compare-flasher-candidates.py"),
                    "--primary",
                    str(primary_archive),
                    "--reproduction",
                    str(reproduction_archive),
                    "--output",
                    str(workspace / "unsigned.tar.gz"),
                    "--report",
                    str(workspace / "report.json"),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("payloads differ", result.stderr)

    def test_candidate_extractors_reject_duplicate_members(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            archive = workspace / "duplicate.tar.gz"
            duplicate_member_archive(archive)

            extracted = run_python(
                "extract-flasher-candidate.py", archive, workspace / "extracted"
            )
            self.assertNotEqual(extracted.returncode, 0)
            self.assertIn("duplicate candidate archive member", extracted.stderr)

            compared = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPTS / "compare-flasher-candidates.py"),
                    "--primary",
                    str(archive),
                    "--reproduction",
                    str(archive),
                    "--output",
                    str(workspace / "output.tar.gz"),
                    "--report",
                    str(workspace / "report.json"),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(compared.returncode, 0)
            self.assertIn("duplicate candidate archive member", compared.stderr)

    def test_candidate_extractors_reject_nonempty_destinations(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            source = workspace / "candidate"
            archive = workspace / "candidate.tar.gz"
            candidate(source)
            self.assertEqual(
                run_python("package-flasher-candidate.py", source, archive).returncode,
                0,
            )
            destination = workspace / "destination"
            destination.mkdir()
            marker = destination / "existing.txt"
            marker.write_text("preserve me\n", encoding="utf-8")

            extracted = run_python(
                "extract-flasher-candidate.py", archive, destination
            )
            self.assertNotEqual(extracted.returncode, 0)
            self.assertIn("must be an empty directory", extracted.stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "preserve me\n")

            spec = importlib.util.spec_from_file_location(
                "compare_flasher_candidates",
                SCRIPTS / "compare-flasher-candidates.py",
            )
            self.assertIsNotNone(spec)
            self.assertIsNotNone(spec.loader if spec else None)
            module = importlib.util.module_from_spec(spec)
            assert spec is not None and spec.loader is not None
            spec.loader.exec_module(module)
            with self.assertRaisesRegex(ValueError, "must be an empty directory"):
                module.extract(archive, destination)
            self.assertEqual(marker.read_text(encoding="utf-8"), "preserve me\n")

    def test_workflow_action_and_tool_pin_contract_is_self_consistent(self) -> None:
        result = run_python("validate-release-workflows.py")
        self.assertEqual(result.returncode, 0, result.stderr)
        candidate_workflow = (
            ROOT / ".github" / "workflows" / "flasher-candidate.yml"
        ).read_text(encoding="utf-8")
        windows_start = candidate_workflow.index("target: x86_64-pc-windows-msvc")
        windows_end = candidate_workflow.index("runs-on:", windows_start)
        self.assertIn("link-arg=/Brepro", candidate_workflow[windows_start:windows_end])

    def test_npm_release_graph_keeps_browser_tools_test_only(self) -> None:
        result = run_python("npm-production-audit.py")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
