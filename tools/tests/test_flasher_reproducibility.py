from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "tools" / "release"
sys.path.insert(0, str(SCRIPTS))

from flasher_build_metadata import (
    EXPECTED_TOOLS,
    EXPECTED_WEB_PACKAGES,
    build_metadata,
    resolved_tools,
    validate_metadata,
)
from flasher_candidate_output import resolve_output
from flasher_reproducibility import validate_report
from flasher_rustdoc import normalize_generic_pages, workspace_package_names
from flasher_sparse_sizes import MERGED_BASELINES, SPARSE_BASELINES, build_report
from source_snapshot import (
    REQUIRED_SOURCE_FILES,
    package_source_snapshot,
    validate_archive_members,
)

VERSION = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
COMMIT = "a" * 40


def tools() -> dict[str, str]:
    return {
        "rustc": "rustc 1.96.0 (fixture)",
        "cargo": "cargo 1.96.0 (fixture)",
        "node": "v24.18.0",
        "npm": "11.0.0",
        "dioxus": "dioxus 0.7.5",
        "wasm_bindgen": "wasm-bindgen 0.2.126",
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
    heltec_r8_size: int = 1_000_000,
    t_beam_size: int = 1_000_000,
    xiao_size: int = 900_000,
    source_size: int | None = None,
) -> dict:
    value = {
        "release": {"version": VERSION, "commit": COMMIT},
        "targets": [
            {
                "board_slug": "heltec-v4",
                "transport": "esp-serial",
                "parts": [{"size": heltec_size}],
                "variants": [],
            },
            {
                "board_slug": "heltec-v4-r8",
                "transport": "esp-serial",
                "parts": [{"size": heltec_r8_size}],
                "variants": [],
            },
            {
                "board_slug": "t-beam-supreme",
                "transport": "esp-serial",
                "parts": [{"size": t_beam_size}],
                "variants": [],
            },
            {
                "board_slug": "xiao-esp32-c6",
                "transport": "esp-serial",
                "parts": [{"size": xiao_size}],
                "variants": [],
            },
            {
                "board_slug": "t-echo",
                "transport": "uf2-mass-storage",
                "parts": [],
                "variants": [{"size": 350_000}, {"size": 350_000}],
            },
            {
                "board_slug": "t114",
                "transport": "uf2-mass-storage",
                "parts": [],
                "variants": [{"size": 350_000}],
            },
            {
                "board_slug": "t096",
                "transport": "uf2-mass-storage",
                "parts": [],
                "variants": [{"size": 350_000}],
            },
            {
                "board_slug": "t1000-e",
                "transport": "nrf-serial-dfu",
                "parts": [],
                "variants": [],
                "nrf_serial_dfu": {
                    "application": {"size": 350_000},
                    "init_packet": {"size": 128},
                    "recovery": {"artifact": {"size": 700_000}},
                },
            },
        ],
    }
    if source_size is not None:
        for target in value["targets"]:
            if target["board_slug"] in {"heltec-v4", "heltec-v4-r8", "t-beam-supreme"}:
                target["source"] = {"size": source_size}
    return value


def run_python(script: str, *arguments: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPTS / script), *(str(value) for value in arguments)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def run_repository_python(
    script: Path, *arguments: Path
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(script), *(str(value) for value in arguments)],
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
                "schema": 3,
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
    release_directory = root / "website" / "releases" / VERSION
    release_directory.mkdir(parents=True)
    history = root / "metadata" / "release-history.json"
    history.parent.mkdir(parents=True)
    history.write_text(
        json.dumps(
            {
                "schema": 1,
                "mode": "bootstrap",
                "head": None,
                "tree": {
                    "file_count": 0,
                    "total_bytes": 0,
                    "tree_sha256": hashlib.sha256(b"").hexdigest(),
                },
                "files": [],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


def duplicate_member_archive(path: Path) -> None:
    with tarfile.open(path, "w:gz") as archive:
        for payload in (b"first", b"second"):
            member = tarfile.TarInfo("payload.txt")
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))


class FlasherReproducibilityTests(unittest.TestCase):
    def test_source_snapshot_is_deterministic_and_covers_both_sites(self) -> None:
        commit = subprocess.run(
            ("git", "rev-parse", "HEAD"),
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            first, first_checksum = package_source_snapshot(
                repository=ROOT,
                commit=commit,
                version=VERSION,
                output=output / "first" / "source.zip",
                metadata=output / "first" / "source.json",
            )
            second, second_checksum = package_source_snapshot(
                repository=ROOT,
                commit=commit,
                version=VERSION,
                output=output / "second" / "source.zip",
                metadata=output / "second" / "source.json",
            )
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(first_checksum.read_bytes(), second_checksum.read_bytes())
            self.assertEqual(
                (output / "first" / "source.json").read_bytes(),
                (output / "second" / "source.json").read_bytes(),
            )
            with zipfile.ZipFile(first) as archive:
                names = set(archive.namelist())
            self.assertFalse(any(name.endswith("/") for name in names))
            prefix = f"Prns-{VERSION}/"
            for relative in REQUIRED_SOURCE_FILES:
                self.assertIn(f"{prefix}{relative}", names)
            self.assertIn(
                f"{prefix}docs/website/src/components/footer.rs",
                names,
            )
            self.assertIn(
                f"{prefix}assets/nnpages/masthead.mu",
                names,
            )

    def test_source_snapshot_is_timezone_independent(self) -> None:
        commit = subprocess.run(
            ("git", "rev-parse", "HEAD"),
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            outputs = []
            for name, timezone in (("utc", "UTC0"), ("est", "EST5")):
                output = root / name
                environment = os.environ.copy()
                environment["TZ"] = timezone
                subprocess.run(
                    (
                        sys.executable,
                        str(SCRIPTS / "package-source-snapshot.py"),
                        "--repository",
                        str(ROOT),
                        "--commit",
                        commit,
                        "--version",
                        VERSION,
                        "--output",
                        str(output / "source.zip"),
                        "--metadata",
                        str(output / "source.json"),
                    ),
                    cwd=ROOT,
                    env=environment,
                    text=True,
                    capture_output=True,
                    check=True,
                )
                outputs.append(
                    (
                        (output / "source.zip").read_bytes(),
                        (output / "source.zip.sha256").read_bytes(),
                        (output / "source.json").read_bytes(),
                    )
                )
            self.assertEqual(outputs[0], outputs[1])

    def test_source_snapshot_rejects_missing_nomadnet_page_source(self) -> None:
        commit = subprocess.run(
            ("git", "rev-parse", "HEAD"),
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        with tempfile.TemporaryDirectory() as temporary:
            archive, _ = package_source_snapshot(
                repository=ROOT,
                commit=commit,
                version=VERSION,
                output=Path(temporary) / "source.zip",
            )
            rewritten = io.BytesIO()
            omitted = (
                f"Prns-{VERSION}/"
                "assets/nnpages/credits.mu"
            )
            with (
                zipfile.ZipFile(archive) as source,
                zipfile.ZipFile(rewritten, mode="w") as destination,
            ):
                for member in source.infolist():
                    if member.filename != omitted:
                        destination.writestr(member, source.read(member))
            with self.assertRaisesRegex(ValueError, "NomadNet sources"):
                validate_archive_members(rewritten.getvalue(), version=VERSION)

    def test_source_snapshot_refuses_to_follow_an_output_symlink(self) -> None:
        commit = subprocess.run(
            ("git", "rev-parse", "HEAD"),
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            protected = root / "protected.zip"
            protected.write_bytes(b"keep me")
            output = root / "source.zip"
            output.symlink_to(protected)
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                package_source_snapshot(
                    repository=ROOT,
                    commit=commit,
                    version=VERSION,
                    output=output,
                )
            self.assertEqual(protected.read_bytes(), b"keep me")

    def test_source_snapshot_is_not_a_website_build_side_effect(self) -> None:
        build_rs = (ROOT / "docs" / "website" / "build.rs").read_text(encoding="utf-8")
        candidate_build = (SCRIPTS / "build-flasher-candidate.sh").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("generate_source_archive", build_rs)
        self.assertNotIn("PRNS_WRITE_PUBLIC_ASSETS", build_rs)
        self.assertIn("package-source-snapshot.py", candidate_build)
        self.assertIn("public/source.zip", candidate_build)
        self.assertLess(
            candidate_build.index("package-source-snapshot.py"),
            candidate_build.index(
                'release firmware build -- --all "$candidate"'
            ),
        )
        self.assertLess(
            candidate_build.index("package-source-snapshot.py"),
            candidate_build.index('cp -R "$hosted_dist/." "$candidate/website/"'),
        )

    def test_candidate_rustdoc_is_deterministically_ordered_and_normalized(
        self,
    ) -> None:
        candidate_build = (SCRIPTS / "build-flasher-candidate.sh").read_text(
            encoding="utf-8"
        )
        metadata = "cargo metadata --locked --no-deps --format-version 1"
        listing = "--list-workspace-packages"
        documentation = 'cargo doc --locked --no-deps --package "$package" --jobs 1'
        normalization = 'flasher_rustdoc.py" \\\n    "$root/target/doc"'
        staging = 'cp -R "$root/target/doc/." "$candidate/website/api/"'
        self.assertIn(metadata, candidate_build)
        self.assertIn(listing, candidate_build)
        self.assertIn(documentation, candidate_build)
        self.assertIn(normalization, candidate_build)
        self.assertNotIn("cargo doc --locked --no-deps --workspace", candidate_build)
        self.assertLess(
            candidate_build.index(metadata),
            candidate_build.index(documentation),
        )
        self.assertLess(
            candidate_build.index(documentation),
            candidate_build.index(normalization),
        )
        self.assertLess(
            candidate_build.index(normalization),
            candidate_build.index(staging),
        )

    def test_rustdoc_generic_page_normalization_is_atomic(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            (output / "docs").mkdir()
            (output / "docs" / "index.html").write_text("docs", encoding="utf-8")
            help_page = output / "help.html"
            settings_page = output / "settings.html"
            help_page.write_text(
                '<meta data-current-crate="first"><main>help</main>',
                encoding="utf-8",
            )
            settings_page.write_text(
                '<meta data-current-crate="second"><main>settings</main>',
                encoding="utf-8",
            )

            normalize_generic_pages(output, "docs")

            self.assertEqual(
                {
                    "help": help_page.read_text(encoding="utf-8"),
                    "settings": settings_page.read_text(encoding="utf-8"),
                },
                {
                    "help": '<meta data-current-crate="docs"><main>help</main>',
                    "settings": '<meta data-current-crate="docs"><main>settings</main>',
                },
            )

            original_help = help_page.read_bytes()
            settings_page.write_text("<main>settings</main>", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "0 current-crate fields"):
                normalize_generic_pages(output, "docs")
            self.assertEqual(help_page.read_bytes(), original_help)

    def test_rustdoc_workspace_packages_are_unique_and_sorted(self) -> None:
        metadata = {
            "packages": [
                {"id": "dependency", "name": "outside"},
                {"id": "second", "name": "z-package"},
                {"id": "first", "name": "a_package"},
            ],
            "workspace_members": ["second", "first"],
        }
        self.assertEqual(
            workspace_package_names(metadata),
            ["a_package", "z-package"],
        )

        metadata["workspace_members"] = ["first", "dependency", "second"]
        metadata["packages"][0]["name"] = "a_package"
        with self.assertRaisesRegex(ValueError, "repeats a workspace package name"):
            workspace_package_names(metadata)

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

    def test_build_metadata_uses_each_tools_version_report_flag(self) -> None:
        commands: list[tuple[str, ...]] = []

        def output(*command: str) -> str:
            commands.append(command)
            return command[0]

        with patch(
            "flasher_build_metadata.resolved_llvm_objcopy",
            return_value="llvm-objcopy version 20.1.8",
        ):
            resolved_tools(output=output)

        self.assertEqual(
            commands,
            [
                ("rustc", "--version"),
                ("cargo", "--version"),
                ("node", "--version"),
                ("npm", "--version"),
                ("dx", "--version"),
                ("wasm-bindgen", "--version"),
                ("cargo-binstall", "-V"),
                ("espup", "--version"),
                ("rustc", "+esp", "--version"),
                ("xtensa-esp-elf-gcc", "--version"),
                ("python3", "--version"),
                ("git", "--version"),
            ],
        )

    def test_sparse_report_covers_all_boards_and_enforces_sixty_percent(self) -> None:
        report = build_report(manifest())
        self.assertEqual(len(report["targets"]), 8)
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
                    heltec_r8_size=SPARSE_BASELINES["heltec-v4-r8"] * 40 // 100,
                    t_beam_size=SPARSE_BASELINES["t-beam-supreme"] * 40 // 100,
                    xiao_size=MERGED_BASELINES["xiao-esp32-c6"],
                )
            )

    def test_sparse_report_excludes_one_embedded_archive_from_s3_code_gate(self) -> None:
        source_size = 5_000_000
        report = build_report(
            manifest(
                heltec_size=6_000_000,
                heltec_r8_size=6_000_000,
                t_beam_size=6_000_000,
                source_size=source_size,
            )
        )
        heltec = next(
            target for target in report["targets"] if target["board_slug"] == "heltec-v4"
        )
        self.assertEqual(heltec["total_bytes"], 6_000_000)
        self.assertEqual(heltec["embedded_source_bytes"], source_size)
        self.assertEqual(heltec["code_payload_bytes"], 1_000_000)
        self.assertEqual(heltec["gate"], "passed")

    def test_candidate_output_is_absolute_before_directory_changes(self) -> None:
        repository = Path("/workspace/prns")
        resolved = resolve_output(
            repository,
            Path("target/flasher-candidate"),
            cwd=repository,
        )
        self.assertEqual(
            resolved, repository.resolve() / "target" / "flasher-candidate"
        )
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
        identity = (SCRIPTS / "release-esp-toolchain-identity.sh").read_text(
            encoding="utf-8"
        )
        verifier = (SCRIPTS / "verify-release-esp-toolchain.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn('ESP_CROSSTOOL_VERSION="15.2.0_20250920"', identity)
        self.assertIn(
            'gcc_sha256="e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977"',
            installer,
        )
        self.assertIn("--crosstool-toolchain-version", installer)
        self.assertIn(
            'ESP_RUSTC_COMMIT_HASH="95e5bda868c960c607597bc03ed9e8f0ad26226d"',
            identity,
        )
        self.assertIn('ESP_RUSTC_COMMIT_DATE="2026-04-15"', identity)
        self.assertIn("verify-release-esp-toolchain.sh", installer)
        self.assertIn("commit_hash", verifier)
        self.assertIn("commit_date", verifier)
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
        result = run_repository_python(
            ROOT / "validation" / "release" / "workflow-contracts.py"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        for workflow_name in ("flasher-candidate.yml", "prnsd-candidate.yml"):
            workflow = (
                ROOT / ".github" / "workflows" / workflow_name
            ).read_text(encoding="utf-8")
            windows_start = workflow.index("target: x86_64-pc-windows-msvc")
            windows_end = workflow.index("runs-on:", windows_start)
            windows_matrix = workflow[windows_start:windows_end]
            self.assertIn("linker=rust-lld", windows_matrix)
            self.assertIn("linker-flavor=lld-link", windows_matrix)
            self.assertIn("link-arg=/Brepro", windows_matrix)
            self.assertIn("link-arg=/debug:none", windows_matrix)
            self.assertIn(
                "Expose and prove the Rust-bundled Windows linker",
                workflow,
            )
            self.assertIn(
                "${sysroot}/lib/rustlib/${host}/bin/rust-lld.exe",
                workflow,
            )
            self.assertIn(r"grep -E '^LLD 22\.1\.2 '", workflow)
            self.assertIn("Reject unshipped Windows PDB identity", workflow)
            self.assertIn("--coff-debug-directory", workflow)
            self.assertIn("PDBFileName:", workflow)

    def test_release_builds_use_catalog_driven_all_board_selection(self) -> None:
        shipping = (
            ROOT / "validation" / "platforms" / "shipping-firmware.sh"
        ).read_text(encoding="utf-8")
        candidate = (
            ROOT / "tools" / "release" / "build-flasher-candidate.sh"
        ).read_text(encoding="utf-8")
        builder = (
            ROOT / "tools" / "release" / "build-flash-artifact.sh"
        ).read_text(encoding="utf-8")
        all_board_build = 'release firmware build -- --all'
        self.assertIn(all_board_build, shipping)
        self.assertIn(all_board_build, candidate)
        self.assertIn(
            "cargo run --quiet --locked -p hopspot-flash -- list",
            builder,
        )
        self.assertIn("awk '{print $1}'", builder)
        self.assertIn("--all)", builder)
        self.assertNotIn("for board in heltec-v4", shipping)
        self.assertNotIn("for board in heltec-v4", candidate)
        self.assertNotIn("PRNS_EMBEDDED_SITE", shipping)

    def test_embedded_captive_portal_is_a_bounded_static_asset(self) -> None:
        portal = (
            ROOT
            / "personal-hopspot"
            / "embedded"
            / "esp32"
            / "assets"
            / "captive-portal.html"
        ).read_bytes()
        self.assertLessEqual(len(portal), 2_048)
        self.assertIn(b"Hopspot is ready", portal)
        self.assertIn(b"does not provide Internet access", portal)
        lowered = portal.lower()
        for forbidden in (b"<script", b"href=", b"src=", b"source.zip", b"dioxus"):
            self.assertNotIn(forbidden, lowered)

    def test_npm_release_graph_keeps_browser_tools_test_only(self) -> None:
        result = run_repository_python(
            ROOT / "validation" / "security" / "npm-production-audit.py"
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
