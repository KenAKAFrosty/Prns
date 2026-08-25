from __future__ import annotations

from contextlib import redirect_stderr
import importlib.util
from io import StringIO
import json
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
            ("ESP32-S3 Heltec", "ESP32-S3 Heltec R8", "ESP32-S3 T-Beam"),
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


if __name__ == "__main__":
    unittest.main()
