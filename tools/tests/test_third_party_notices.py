from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
from io import StringIO
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "tools" / "repo" / "generate-third-party-notices.py"
SPEC = importlib.util.spec_from_file_location("third_party_notices", GENERATOR)
assert SPEC is not None and SPEC.loader is not None
notices = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(notices)


class ThirdPartyNoticeTests(unittest.TestCase):
    def test_app_graphs_add_only_the_platform_product_features(self) -> None:
        graphs = {name: (manifest, target) for name, manifest, target in notices.GRAPHS}
        self.assertEqual(len(graphs), len(notices.GRAPHS))
        self.assertEqual(
            notices.GRAPH_FEATURES,
            {"Prns app Android": ("android",), "Prns app iOS": ("apple",)},
        )
        app_manifest = "applications/prns/native-composition/Cargo.toml"
        self.assertEqual(graphs["Prns app Android"], (app_manifest, "aarch64-linux-android"))
        self.assertEqual(graphs["Prns app iOS"], (app_manifest, "aarch64-apple-ios"))
        defaults = {name: graph for name, graph in graphs.items() if name not in notices.GRAPH_FEATURES}
        self.assertEqual(len(defaults), 28)
        self.assertEqual(defaults["engine"], ("Cargo.toml", "x86_64-unknown-linux-gnu"))
        self.assertEqual(
            defaults["Android"],
            ("personal-hopspot/mobile/android/rust/Cargo.toml", "aarch64-linux-android"),
        )
        self.assertEqual(
            defaults["iOS"],
            ("personal-hopspot/mobile/ios/rust/Cargo.toml", "aarch64-apple-ios"),
        )

    def test_input_fingerprint_covers_checked_in_manifests_locks_and_notice_sources(
        self,
    ) -> None:
        relative = {
            path.relative_to(ROOT).as_posix()
            for path in notices.notice_input_paths()
        }

        self.assertIn("Cargo.toml", relative)
        self.assertIn("Cargo.lock", relative)
        self.assertIn("prnsd/Cargo.toml", relative)
        self.assertIn("prnsd/Cargo.lock", relative)
        self.assertIn("about.toml", relative)
        self.assertIn("docs/website/package-lock.json", relative)
        self.assertIn("release/licenses/pako-Zlib.txt", relative)
        self.assertIn("release/licenses/mbedtls-Apache-2.0.txt", relative)
        self.assertNotIn("docs/website/node_modules/atob-lite/LICENSE.md", relative)
        self.assertFalse(any("node_modules" in Path(path).parts for path in relative))

    def test_fast_input_check_accepts_the_exact_fingerprint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "THIRD_PARTY_NOTICES.md"
            output.write_text(
                f"{notices.INPUT_FINGERPRINT_PREFIX}{notices.notice_input_fingerprint()}`.\n",
                encoding="utf-8",
            )
            stdout = StringIO()
            with redirect_stdout(stdout):
                self.assertTrue(notices.check_notice_inputs(output))

        self.assertIn("input fingerprint matches", stdout.getvalue())

    def test_fast_input_check_rejects_stale_or_missing_fingerprints(self) -> None:
        for content, diagnostic in (
            (
                f"{notices.INPUT_FINGERPRINT_PREFIX}{'0' * 64}`.\n",
                "notice inputs changed",
            ),
            ("# Third-Party Notices\n", "has no input fingerprint"),
        ):
            with self.subTest(diagnostic=diagnostic), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "THIRD_PARTY_NOTICES.md"
                output.write_text(content, encoding="utf-8")
                stderr = StringIO()
                with redirect_stderr(stderr):
                    self.assertFalse(notices.check_notice_inputs(output))
                self.assertIn(diagnostic, stderr.getvalue())

    def test_fast_input_mode_does_not_generate_the_notice_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "THIRD_PARTY_NOTICES.md"
            output.write_text(
                f"{notices.INPUT_FINGERPRINT_PREFIX}{notices.notice_input_fingerprint()}`.\n",
                encoding="utf-8",
            )
            with (
                mock.patch.object(notices, "notice_bundle") as generate,
                mock.patch.object(
                    sys,
                    "argv",
                    ["generator", "--check-inputs", "--output", str(output)],
                ),
                redirect_stdout(StringIO()),
            ):
                self.assertEqual(notices.main(), 0)
            generate.assert_not_called()

    def test_esp32s3_mbedtls_archive_is_in_the_vendored_inventory(self) -> None:
        entries = {
            package: (identifier, relative, graphs)
            for package, identifier, relative, graphs in notices.VENDORED
        }
        package = "Mbed TLS ffb280bb63c78bfec1e1ab55040671768c85c923"

        self.assertEqual(entries[package][0], "Apache-2.0")
        self.assertEqual(
            entries[package][1], "release/licenses/mbedtls-Apache-2.0.txt"
        )
        self.assertEqual(
            entries[package][2],
            (
                "ESP32-S3 Heltec",
                "ESP32-S3 Heltec E290",
                "ESP32-S3 Heltec R8",
                "ESP32-S3 T-Beam",
            ),
        )

    def test_notice_text_normalizes_presentation_only_whitespace(self) -> None:
        source = (
            "Copyright Example  \r\n"
            "\r\n"
            " \r\n"
            "Permission is granted.\t\r\n"
            "\r\n"
        )

        self.assertEqual(
            notices.normalized_notice_text(source),
            "Copyright Example\n\nPermission is granted.",
        )

    def test_fetch_uses_the_complete_locked_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cargo_home = Path(temporary)
            with mock.patch.object(notices.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, "", "")

                notices.fetch_manifest("prnsd/Cargo.toml", cargo_home)

        command = run.call_args.args[0]
        self.assertEqual(command[:3], ["cargo", "fetch", "--locked"])
        self.assertEqual(
            command[command.index("--manifest-path") + 1],
            str(ROOT / "prnsd/Cargo.toml"),
        )
        self.assertNotIn("--target", command)
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_HOME"], str(cargo_home))
        self.assertNotIn("--features", command)
        self.assertNotIn("--all-features", command)
        self.assertNotIn("--no-default-features", command)

    def test_generation_forwards_selected_features_without_disabling_defaults(self) -> None:
        def complete(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            output = Path(command[command.index("--output-file") + 1])
            output.write_text(json.dumps({"licenses": []}), encoding="utf-8")
            return subprocess.CompletedProcess(command, 0, "", "")

        for features in (("android",), ("apple",), ("first", "second")):
            with self.subTest(features=features), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                with mock.patch.object(notices.subprocess, "run", side_effect=complete) as run:
                    notices.generate_graph(
                        "applications/prns/native-composition/Cargo.toml",
                        "aarch64-apple-ios",
                        directory,
                        directory,
                        "/tools/cargo-about",
                        features=features,
                    )

                command = run.call_args.args[0]
                self.assertEqual(command[command.index("--features") + 1], " ".join(features))
                self.assertNotIn("--all-features", command)
                self.assertNotIn("--no-default-features", command)

    def test_bundle_uses_and_records_each_graphs_feature_selection(self) -> None:
        data = {"licenses": [{
            "id": "LicenseRef-Nordic-SoftDevice",
            "name": "test license",
            "text": "test notice",
        }]}
        with (
            mock.patch.object(notices, "about_binary", return_value="/tools/cargo-about"),
            mock.patch.object(notices, "about_version", return_value="cargo-about 0.9.1"),
            mock.patch.object(notices, "notice_input_fingerprint", return_value="0" * 64),
            mock.patch.object(notices, "fetch_manifest") as fetch,
            mock.patch.object(notices, "generate_graph", return_value=data) as generate,
            mock.patch.object(notices, "NPM", ()),
            mock.patch.object(notices, "VENDORED", ()),
        ):
            rendered = notices.notice_bundle()

        self.assertEqual(generate.call_count, len(notices.GRAPHS))
        for (name, manifest, target), call in zip(notices.GRAPHS, generate.call_args_list):
            features = notices.GRAPH_FEATURES.get(name, ())
            self.assertEqual(call.args[:2], (manifest, target))
            self.assertEqual(call.kwargs, {"features": features})
            if features:
                self.assertIn(
                    f"- {name}: `{manifest}` (`{target}`, locked resolution, "
                    f"default features plus `{features[0]}`)",
                    rendered,
                )
            else:
                self.assertIn(f"- {name}: `{manifest}` (`{target}`, locked resolution)", rendered)
        self.assertEqual(
            [call.args[0] for call in fetch.call_args_list].count(
                "applications/prns/native-composition/Cargo.toml"
            ),
            1,
        )

    @unittest.skipUnless(
        os.name == "posix" and Path("/bin/bash").is_file(),
        "the release-audit shell regression requires a POSIX host with /bin/bash",
    )
    def test_dependency_audit_forwards_the_same_product_graph_selections(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            security = root / "validation/security"
            security.mkdir(parents=True)
            script = security / "deps-audit.sh"
            script.write_bytes((ROOT / "validation/security/deps-audit.sh").read_bytes())
            binaries = root / "bin"
            binaries.mkdir()
            cargo = binaries / "cargo"
            cargo.write_text(
                '#!/bin/sh\ncase "$*" in\n'
                '"deny --version") echo "cargo-deny 0.19.8" ;;\n'
                '"about --version") echo "cargo-about 0.9.1" ;;\n'
                '*) printf "CALL\\n" >> "$NOTICE_TEST_CARGO_LOG"; '
                'printf "%s\\n" "$@" >> "$NOTICE_TEST_CARGO_LOG" ;;\nesac\n',
                encoding="utf-8",
            )
            cargo.chmod(0o755)
            tools = root / "tools"
            tools.mkdir()
            for stub in (binaries / "python3", binaries / "npm", tools / "prns"):
                stub.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                stub.chmod(0o755)
            cargo_log = root / "cargo-calls.txt"
            result = subprocess.run(
                ["/bin/bash", str(script)],
                env={
                    **os.environ,
                    "PATH": f"{binaries}:/usr/bin:/bin",
                    "NOTICE_TEST_CARGO_LOG": str(cargo_log),
                },
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            commands = [call.splitlines() for call in cargo_log.read_text().split("CALL\n")[1:]]
            graphs = set()
            for command in commands:
                self.assertEqual(command[0], "deny")
                self.assertIn("--locked", command)
                self.assertIn("--exclude-dev", command)
                self.assertNotIn("--all-features", command)
                self.assertNotIn("--no-default-features", command)
                manifest = Path(command[command.index("--manifest-path") + 1]).relative_to(root).as_posix()
                target = command[command.index("--target") + 1]
                features = (
                    tuple(command[command.index("--features") + 1].split())
                    if "--features" in command else ()
                )
                graphs.add((manifest, target, features))
            # The pre-existing native Host SDK notice graph is not a separate
            # release-audit root. All other graphs retain matching selections.
            expected = {
                (manifest, target, notices.GRAPH_FEATURES.get(name, ()))
                for name, manifest, target in notices.GRAPHS
                if name != "Host SDK native"
            }
            self.assertEqual(graphs, expected)
            self.assertEqual(len(commands), len(expected))
            self.assertIn("prns-app-android (aarch64-linux-android, default features plus android)", result.stdout)
            self.assertIn("prns-app-ios (aarch64-apple-ios, default features plus apple)", result.stdout)

    def test_generation_is_locked_offline_and_target_explicit(self) -> None:
        def complete(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            output = Path(command[command.index("--output-file") + 1])
            output.write_text(json.dumps({"licenses": []}), encoding="utf-8")
            return subprocess.CompletedProcess(command, 0, "", "")

        with tempfile.TemporaryDirectory() as temporary:
            cargo_home = Path(temporary) / "cargo-home"
            cargo_home.mkdir()
            output = Path(temporary) / "output"
            output.mkdir()
            with mock.patch.object(notices.subprocess, "run", side_effect=complete) as run:
                result = notices.generate_graph(
                    "prnsd/Cargo.toml",
                    "x86_64-unknown-linux-gnu",
                    output,
                    cargo_home,
                    "/tools/cargo-about",
                )

        self.assertEqual(result, {"licenses": []})
        command = run.call_args.args[0]
        self.assertEqual(command[0], "/tools/cargo-about")
        self.assertIn("--locked", command)
        self.assertIn("--offline", command)
        self.assertEqual(command[command.index("--config") + 1], str(ROOT / "about.toml"))
        self.assertEqual(
            command[command.index("--target") + 1],
            "x86_64-unknown-linux-gnu",
        )
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_HOME"], str(cargo_home))

        self.assertNotIn("--features", command)
        self.assertNotIn("--all-features", command)
        self.assertNotIn("--no-default-features", command)

    def test_mismatch_reports_the_exact_unified_diff(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "THIRD_PARTY_NOTICES.md"
            output.write_text("old notice\n", encoding="utf-8")
            stderr = StringIO()
            with (
                mock.patch.object(notices, "notice_bundle", return_value="new notice\n"),
                mock.patch.object(sys, "argv", ["generator", "--output", str(output)]),
                redirect_stderr(stderr),
            ):
                result = notices.main()

        self.assertEqual(result, 1)
        diagnostic = stderr.getvalue()
        self.assertIn(f"--- {output} (committed)", diagnostic)
        self.assertIn(f"+++ {output} (generated)", diagnostic)
        self.assertIn("-old notice", diagnostic)
        self.assertIn("+new notice", diagnostic)


if __name__ == "__main__":
    unittest.main()
