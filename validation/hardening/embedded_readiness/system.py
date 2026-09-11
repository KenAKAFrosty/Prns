from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

from validation.hardening.embedded_isa.contract import HostPlatform
from validation.hardening.embedded_readiness.contract import ROOT
from validation.hardening.embedded_readiness.model import CommandOutput


class SystemProbe:
    def host_platform(self) -> HostPlatform | None:
        machine = platform.machine().lower()
        if sys.platform == "darwin":
            if machine in {"arm64", "aarch64"}:
                return HostPlatform.MACOS_ARM64
            if machine in {"amd64", "x86_64"}:
                return HostPlatform.MACOS_AMD64
        if sys.platform.startswith("linux"):
            if machine in {"arm64", "aarch64"}:
                return HostPlatform.LINUX_ARM64
            if machine in {"amd64", "x86_64"}:
                return HostPlatform.LINUX_AMD64
        if sys.platform == "win32" and machine in {"amd64", "x86_64"}:
            return HostPlatform.WINDOWS_AMD64
        return None

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
