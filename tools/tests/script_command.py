"""Build explicit interpreter commands for release scripts on every supported host.

Unix can execute a script through its shebang, but Windows callers must name the interpreter.
"""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import sys


def _windows_bash() -> str:
    """Locate the Git for Windows bash whose environment includes MSYS coreutils.

    `shutil.which("bash")` can resolve to the WSL launcher, which cannot consume native Windows paths.
    Prefer the installation containing the selected `git`, then check the standard Git for Windows roots.
    """
    roots: list[Path] = []
    git = shutil.which("git")
    if git is not None:
        roots.append(Path(git).resolve().parents[1])
    for variable in ("ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"):
        base = os.environ.get(variable)
        if base:
            roots.append(Path(base) / "Git")
    for root in roots:
        candidate = root / "bin" / "bash.exe"
        if candidate.is_file():
            return str(candidate)
    raise RuntimeError(
        "Git for Windows bash is required to run the release shell scripts; "
        "looked beside git and under " + ", ".join(str(root) for root in roots)
    )


def script_command(target: Path, *arguments: object) -> list[str]:
    """Build a complete command for one supported repository script."""
    if target.suffix == ".py":
        interpreter = sys.executable
    elif target.suffix == ".sh":
        if os.name == "nt":
            interpreter = _windows_bash()
        else:
            interpreter = shutil.which("bash")
            if interpreter is None:
                raise RuntimeError("bash is required to run the release shell scripts")
    else:
        raise RuntimeError(f"unsupported repository script type: {target}")
    return [interpreter, str(target), *(str(argument) for argument in arguments)]
