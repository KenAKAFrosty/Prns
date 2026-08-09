from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from script_command import _windows_bash, script_command


class ScriptCommandTests(unittest.TestCase):
    def test_python_command_couples_the_interpreter_script_and_arguments(self) -> None:
        target = Path("tools/release/example.py")
        self.assertEqual(
            script_command(target, Path("output"), 7),
            [sys.executable, str(target), "output", "7"],
        )

    def test_unknown_script_types_fail_closed(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "unsupported repository script type"):
            script_command(Path("tools/release/example"))

    def test_windows_bash_follows_the_git_installation_on_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "Git"
            git = root / "cmd" / "git.exe"
            bash = root / "bin" / "bash.exe"
            git.parent.mkdir(parents=True)
            bash.parent.mkdir(parents=True)
            git.touch()
            bash.touch()
            with patch("script_command.shutil.which", return_value=str(git)):
                self.assertEqual(_windows_bash(), str(bash.resolve()))


if __name__ == "__main__":
    unittest.main()
