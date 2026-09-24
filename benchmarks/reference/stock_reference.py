#!/usr/bin/env python3
"""Load and attest the locked stock interpreted RNS benchmark reference."""

from __future__ import annotations

import importlib.metadata
import json
from pathlib import Path
import runpy
import sys


REFERENCE_DIR = Path(__file__).resolve().parent
REPO_ROOT = REFERENCE_DIR.parent.parent
PROOF_DIR = REFERENCE_DIR / ".reference-state"
EXPECTED_RNS_VERSION = "1.5.4"
NATIVE_SUFFIXES = {".so", ".pyd", ".dylib"}


def portable_path(path: Path) -> str:
    resolved = path.resolve()
    if resolved.is_relative_to(REPO_ROOT):
        return "<repo>/" + resolved.relative_to(REPO_ROOT).as_posix()
    home = Path.home().resolve()
    if resolved.is_relative_to(home):
        return "~/" + resolved.relative_to(home).as_posix()
    return resolved.name


def load_stock_rns():
    import RNS
    from cryptography.hazmat.backends.openssl.backend import backend

    version = getattr(RNS, "__version__", None) or getattr(RNS, "VERSION", None)
    if str(version) != EXPECTED_RNS_VERSION:
        raise SystemExit(
            f"stock reference requires RNS {EXPECTED_RNS_VERSION}, loaded {version!r}"
        )
    if getattr(RNS, "compiled", False) is not False:
        raise SystemExit("stock reference requires interpreted RNS.compiled == false")

    rns_modules = {
        name: module
        for name, module in sys.modules.items()
        if (name == "RNS" or name.startswith("RNS."))
        and getattr(module, "__file__", "")
    }
    native_modules = sorted(
        {
            portable_path(Path(module.__file__))
            for module in rns_modules.values()
            if Path(module.__file__).suffix in NATIVE_SUFFIXES
        }
    )
    if native_modules:
        raise SystemExit(
            "stock reference loaded native RNS modules: " + ", ".join(native_modules)
        )
    interpreted_modules = sorted(rns_modules)
    proof = {
        "rns": str(version),
        "mode": "interpreted",
        "compiled": False,
        "native_modules": native_modules,
        "native_module_count": 0,
        "interpreted_modules": interpreted_modules,
        "interpreted_module_count": len(interpreted_modules),
        "python": sys.version.split()[0],
        "cryptography": importlib.metadata.version("cryptography"),
        "openssl": backend.openssl_version_text(),
        "package": portable_path(Path(RNS.__file__)),
    }
    PROOF_DIR.mkdir(parents=True, exist_ok=True)
    (PROOF_DIR / "proof.json").write_text(
        json.dumps(proof, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print("REFERENCE_PROOF " + json.dumps(proof, sort_keys=True), flush=True)
    return proof


def main() -> None:
    load_stock_rns()
    if sys.argv[1:] == ["--verify-only"]:
        return
    if len(sys.argv) < 2:
        raise SystemExit("usage: stock_reference.py <script.py> [args ...]")
    script = Path(sys.argv[1]).resolve()
    sys.argv = [str(script), *sys.argv[2:]]
    runpy.run_path(str(script), run_name="__main__")


if __name__ == "__main__":
    main()
