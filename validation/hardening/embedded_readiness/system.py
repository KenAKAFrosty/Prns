from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

from validation.hardening.embedded_readiness.contract import ROOT
from validation.hardening.embedded_readiness.model import CommandOutput


class SystemProbe:
    def find(self, command: str, search_paths: tuple[Path, ...] = ()) -> Path | None:
        path = None
        if search_paths:
            path = shutil.which(
                command,
                path=os.pathsep.join(str(entry) for entry in search_paths),
            )
        if path is None:
            path = shutil.which(command)
        return Path(path).resolve() if path is not None else None

    def run(self, command: tuple[str, ...]) -> CommandOutput:
        try:
            result = subprocess.run(
                command,
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as error:
            return CommandOutput(127, "", str(error))
        return CommandOutput(result.returncode, result.stdout, result.stderr)
