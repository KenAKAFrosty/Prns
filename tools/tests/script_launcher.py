"""Resolve the interpreter a release script needs, so this suite runs on Windows too.

Only Unix runs a script through its shebang, so every caller has to name the interpreter.
Naming the shell is the part with a trap in it, and it is worth stating once here rather than
in each caller.
"""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import sys


def _windows_bash() -> str:
    """Locate the Git for Windows bash that can actually run a repository script.

    `shutil.which("bash")` is not safe on Windows. Where WSL is installed it resolves to
    `C:\\Windows\\System32\\bash.exe`, the WSL launcher, which cannot see a `C:` path at all and
    fails with 127 no matter how the path is spelled.

    Git ships two bash binaries and only one of them is usable. `Git\\bin\\bash.exe` puts the
    MSYS coreutils on PATH; `Git\\usr\\bin\\bash.exe` starts but cannot find `dirname`, so a
    script dies on its first pipeline instead of at its entry point.

    Prefer the bash that sits beside the `git` already on PATH, so a portable or non-default Git
    install is honoured, and fall back to the well-known locations.
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


def script_launcher(target: Path) -> list[str]:
    """Return the argv prefix that runs `target`."""
    if target.suffix != ".sh":
        return [sys.executable]
    if os.name == "nt":
        return [_windows_bash()]
    bash = shutil.which("bash")
    if bash is None:
        raise RuntimeError("bash is required to run the release shell scripts")
    return [bash]
