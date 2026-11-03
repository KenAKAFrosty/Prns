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

    def test_cargo_inputs_exclude_untracked_nested_checkouts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            for relative in (
                "Cargo.toml", "Cargo.lock", "member/Cargo.toml",
                "vendor/example/Cargo.toml", "scratch/Cargo.toml",
                ".claude/worktrees/example/Cargo.toml",
                ".claude/worktrees/example/Cargo.lock",
            ):
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("", encoding="utf-8")
            subprocess.run(
                ["git", "add", "Cargo.toml", "Cargo.lock", "member", "vendor"],
                cwd=root,
                check=True,
            )
            with (
                mock.patch.object(notices, "ROOT", root),
                mock.patch.object(notices.runtime_notices, "input_paths", return_value=()),
                mock.patch.object(notices, "ABOUT", root / "about.toml"),
                mock.patch.object(notices, "__file__", str(root / "generator.py")),
            ):
                actual = {
                    path.relative_to(root).as_posix()
                    for path in notices.notice_input_paths()
                    if path.name in {"Cargo.toml", "Cargo.lock"}
                }
            self.assertEqual(actual, {
                "Cargo.toml", "Cargo.lock", "member/Cargo.toml",
            })

    def test_unavailable_tracked_inventory_fails_closed(self) -> None:
        with mock.patch.object(notices.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 1, b"", b"not a repository")
            with self.assertRaisesRegex(RuntimeError, "cannot enumerate tracked notice inputs"):
                notices.notice_input_paths()

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


    def test_missing_required_peers_remain_prominent_in_rendered_notices(self) -> None:
            npm = {'packages': [], 'unresolved_optional': [], 'unselected_optional_peers': [],
                   'unresolved_required_peers': [['app', 'node_modules/drawer', 'gesture', '>=2']]}
            maven = {'packages': [], 'manual_review': {'summary': 'Pending review.',
                     'declarations': [{'name': 'ML Kit Terms of Service',
                                       'coordinates': ['com.google.mlkit:common:18.11.0']}]}}
            rendered = '\n'.join(notices.app_notice_lines(npm, maven))
            self.assertIn('**Manual release-policy review pending:** Pending review.', rendered)
            self.assertIn('**Manual review — ML Kit Terms of Service:** `com.google.mlkit:common:18.11.0`', rendered)
            self.assertNotIn('**Unresolved release review:**', rendered)
            self.assertIn('1 unresolved required peer declarations', rendered)
            self.assertIn('**Unresolved required peer:** `app/node_modules/drawer` requires `gesture@>=2`', rendered)
            self.assertIn('Missing required peers remain release-audit failures.', rendered)
            self.assertIn('does not prove runtime dependency completeness', rendered)

    def test_packaged_maven_text_is_deduplicated_deterministically_without_license_inference(self) -> None:
            rows = [
                {'coordinate': 'a:one:1', 'artifact':'one.aar', 'name':'Bundled component',
                 'text':'Copyright owner\r\nPermission text  \n', 'profiles':['releaseRuntimeClasspath']},
                {'coordinate': 'b:two:2', 'artifact':'two.aar', 'name':'Other component',
                 'text':'Copyright owner\nPermission text\n', 'profiles':['debugRuntimeClasspath']},
            ]
            first, second = {}, {}
            notices.add_maven_notices(first, {'notices':rows})
            notices.add_maven_notices(second, {'notices':list(reversed(rows))})
            self.assertEqual(len(first), 1)
            self.assertEqual(notices.license_text_lines(first), notices.license_text_lines(second))
            rendered='\n'.join(notices.license_text_lines(first))
            self.assertIn('policy review remains unresolved', rendered)
            self.assertIn('a:one:1 / Bundled component', rendered)
            self.assertIn('application Android releaseRuntimeClasspath', rendered)
            self.assertNotIn('MIT', rendered)

    def test_composed_and_app_images_keep_independent_shipping_feature_graphs(self) -> None:
            graphs = {name: (manifest, target) for name, manifest, target in notices.GRAPHS}
            self.assertEqual(graphs['Service runtime Linux'], ("prns-service-runtime/bindings/uniffi/image/Cargo.toml", 'x86_64-unknown-linux-gnu'))
            self.assertNotIn('Service runtime Linux', notices.GRAPH_FEATURES)
            self.assertEqual(graphs['Service runtime selected Linux'], graphs['Service runtime Linux'])
            self.assertEqual(notices.GRAPH_FEATURES['Service runtime selected Linux'], ("archive", "text", "contact-card", "connections"))
            self.assertEqual(graphs['Service runtime macOS'], ("prns-service-runtime/bindings/uniffi/image/Cargo.toml", 'aarch64-apple-darwin'))
            self.assertNotIn('Service runtime macOS', notices.GRAPH_FEATURES)
            self.assertEqual(graphs['Service runtime selected macOS'], graphs['Service runtime macOS'])
            self.assertEqual(notices.GRAPH_FEATURES['Service runtime selected macOS'], ("archive", "text", "contact-card", "connections"))
            self.assertEqual(graphs['App Android'], ("applications/prns/chat-app/Cargo.toml", 'aarch64-linux-android'))
            self.assertNotIn('App Android', notices.GRAPH_FEATURES)
            self.assertEqual(graphs['App iOS'], ("applications/prns/chat-app/Cargo.toml", 'aarch64-apple-ios'))
            self.assertNotIn('App iOS', notices.GRAPH_FEATURES)

    def test_app_runtime_and_SDK_audit_preserve_target_feature_selections(self) -> None:
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
                # App graphs and inherited runtime/SDK selections
                # must retain the same target and feature selection in the audit.
                expected = {
                    (manifest, target, notices.GRAPH_FEATURES.get(name, ()))
                    for name, manifest, target in notices.GRAPHS
                    if name.startswith(("App ", "Service runtime ", "Expo "))
                }
                self.assertTrue(expected <= graphs)
                self.assertEqual(len(commands), len(graphs))
                self.assertIn("app-android (aarch64-linux-android)", result.stdout)
                self.assertIn("app-ios (aarch64-apple-ios)", result.stdout)


if __name__ == "__main__":
    unittest.main()
