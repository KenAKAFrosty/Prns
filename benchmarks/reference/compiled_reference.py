#!/usr/bin/env python3
"""Load RNS 1.4.2 from a controlled, machine-local Cython object cache."""

from __future__ import annotations

import json
import os
from pathlib import Path
import runpy
import subprocess
import sys


REFERENCE_DIR = Path(__file__).resolve().parent
REPO_ROOT = REFERENCE_DIR.parent.parent
PROOF_DIR = REFERENCE_DIR / ".object-cache"
if os.name == "nt":
    OBJECT_CACHE = Path.home() / ".prns-oc"
else:
    OBJECT_CACHE = PROOF_DIR / "pyximport"


def portable_path(path: Path) -> str:
    resolved = path.resolve()
    if resolved.is_relative_to(REPO_ROOT):
        return "<repo>/" + resolved.relative_to(REPO_ROOT).as_posix()
    home = Path.home().resolve()
    if resolved.is_relative_to(home):
        return "~/" + resolved.relative_to(home).as_posix()
    return resolved.name


def first_line(command: list[str]) -> str:
    try:
        output = subprocess.run(
            command,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        ).stdout
        return output.splitlines()[0].strip()
    except (OSError, subprocess.CalledProcessError, IndexError):
        return "unknown"


def load_compiled_rns():
    import Cython
    import pyximport

    OBJECT_CACHE.mkdir(parents=True, exist_ok=True)
    pyximport.install(
        build_dir=str(OBJECT_CACHE),
        pyimport=True,
        language_level=3,
        inplace=False,
    )
    import RNS

    native_modules = sorted(
        {
            portable_path(Path(module.__file__))
            for name, module in sys.modules.items()
            if (name == "RNS" or name.startswith("RNS."))
            and getattr(module, "__file__", "")
            and Path(module.__file__).suffix in {".so", ".pyd", ".dylib"}
        }
    )
    version = getattr(RNS, "__version__", None) or getattr(RNS, "VERSION", None)
    compiled = getattr(RNS, "compiled", False) is True
    if str(version) != "1.4.2":
        raise SystemExit(f"compiled reference requires RNS 1.4.2, loaded {version!r}")
    if not compiled:
        raise SystemExit("compiled reference requires RNS.compiled == true")
    if not native_modules:
        raise SystemExit("compiled reference loaded no native RNS extension module")

    proof = {
        "rns": str(version),
        "compiled": compiled,
        "native_module": native_modules[0],
        "python": sys.version.split()[0],
        "cython": Cython.__version__,
        "compiler": first_line([os.environ.get("CC", "cc"), "--version"]),
        "object_cache": portable_path(OBJECT_CACHE),
    }
    PROOF_DIR.mkdir(parents=True, exist_ok=True)
    (PROOF_DIR / "proof.json").write_text(
        json.dumps(proof, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print("REFERENCE_PROOF " + json.dumps(proof, sort_keys=True), flush=True)
    return proof


def main() -> None:
    load_compiled_rns()
    if sys.argv[1:] == ["--verify-only"]:
        return
    if len(sys.argv) < 2:
        raise SystemExit("usage: compiled_reference.py <script.py> [args ...]")
    script = Path(sys.argv[1]).resolve()
    sys.argv = [str(script), *sys.argv[2:]]
    runpy.run_path(str(script), run_name="__main__")


if __name__ == "__main__":
    main()
