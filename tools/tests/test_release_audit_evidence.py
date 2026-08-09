from __future__ import annotations

import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parent))

from script_command import script_command

ROOT = Path(__file__).resolve().parents[2]
WRITER = ROOT / "tools" / "release" / "write-release-audit-evidence.sh"


class ReleaseAuditEvidenceTests(unittest.TestCase):
    def test_writer_creates_the_output_parent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            tools = workspace / "bin"
            tools.mkdir()
            cargo = tools / "cargo"
            cargo.write_text(
                """#!/usr/bin/env bash
set -euo pipefail
case "$*" in
    "deny --version") printf '%s\n' "cargo-deny 0.18.9" ;;
    "about --version") printf '%s\n' "cargo-about 0.8.2" ;;
    *) exit 2 ;;
esac
""",
                encoding="utf-8",
            )
            cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
            output = workspace / "absent" / "nested" / "release-audit-evidence.md"
            environment = dict(os.environ)
            environment["PATH"] = f"{tools}{os.pathsep}{environment['PATH']}"

            result = subprocess.run(
                script_command(WRITER, output),
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("cargo-deny 0.18.9", output.read_text(encoding="utf-8"))
            self.assertIn("cargo-about 0.8.2", output.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
