from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
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


if __name__ == "__main__":
    unittest.main()
