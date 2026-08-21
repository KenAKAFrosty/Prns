"""Deterministic release-build identity and exact production tool validation."""

from __future__ import annotations

from datetime import datetime, timezone
import json
from pathlib import Path
import platform
import re
import subprocess


ROOT = Path(__file__).resolve().parents[2]
EXPECTED_TOOLS = {
    "rustc": "1.96.0",
    "cargo": "1.96.0",
    "node": "24.18.0",
    "dioxus": "0.7.5",
    "wasm_bindgen": "0.2.126",
    "cargo_binstall": "1.21.0",
    "espup": "0.17.1",
    "esp_rustc": "1.95.0",
    "llvm_objcopy": "rust-1.96.0-llvm-tools-preview",
    "xtensa_gcc": "esp-15.2.0_20250920-gcc-15.2.0",
}
EXPECTED_WEB_PACKAGES = {
    "esptool-js": "0.6.0",
    "spark-md5": "3.0.2",
    "esbuild": "0.28.1",
}


def command_output(*command: str) -> str:
    process = subprocess.run(command, text=True, capture_output=True, check=False)
    text = (process.stdout or process.stderr).strip()
    if process.returncode != 0 or not text:
        raise ValueError(f"release tool is unavailable: {' '.join(command)}")
    return text.splitlines()[0]


def resolved_llvm_objcopy() -> str:
    sysroot = Path(command_output("rustc", "--print", "sysroot"))
    process = subprocess.run(
        ("rustc", "-vV"), text=True, capture_output=True, check=False
    )
    if process.returncode != 0:
        raise ValueError("could not resolve the Rust host for llvm-objcopy")
    host = next(
        (
            line.removeprefix("host: ").strip()
            for line in process.stdout.splitlines()
            if line.startswith("host: ")
        ),
        "",
    )
    if not host:
        raise ValueError("rustc -vV did not report a host for llvm-objcopy")
    executable = sysroot / "lib" / "rustlib" / host / "bin" / "llvm-objcopy"
    if not executable.is_file():
        raise ValueError("Rust 1.96.0 llvm-tools-preview does not provide llvm-objcopy")
    return command_output(str(executable), "--version")


def resolved_tools(output=command_output) -> dict[str, str]:
    return {
        "rustc": output("rustc", "--version"),
        "cargo": output("cargo", "--version"),
        "node": output("node", "--version"),
        "npm": output("npm", "--version"),
        "dioxus": output("dx", "--version"),
        "wasm_bindgen": output("wasm-bindgen", "--version"),
        "cargo_binstall": output("cargo-binstall", "-V"),
        "espup": output("espup", "--version"),
        "esp_rustc": output("rustc", "+esp", "--version"),
        "xtensa_gcc": output("xtensa-esp-elf-gcc", "--version"),
        "llvm_objcopy": resolved_llvm_objcopy(),
        "python": output("python3", "--version"),
        "git": output("git", "--version"),
    }


def exact_web_packages(root: Path = ROOT) -> dict[str, str]:
    package = json.loads((root / "docs" / "website" / "package.json").read_text(encoding="utf-8"))
    actual = {
        "esptool-js": package.get("dependencies", {}).get("esptool-js"),
        "spark-md5": package.get("dependencies", {}).get("spark-md5"),
        "esbuild": package.get("devDependencies", {}).get("esbuild"),
    }
    if actual != EXPECTED_WEB_PACKAGES:
        raise ValueError(f"release web tool pins drifted: {actual!r}")
    return actual


def validate_tools(tools: dict[str, str]) -> None:
    checks = {
        "rustc": tools.get("rustc", "").startswith("rustc 1.96.0 "),
        "cargo": tools.get("cargo", "").startswith("cargo 1.96.0 "),
        "node": tools.get("node") == "v24.18.0",
        "dioxus": bool(re.search(r"(?:^|\s)0\.7\.5(?:\s|$)", tools.get("dioxus", ""))),
        "wasm_bindgen": tools.get("wasm_bindgen") == "wasm-bindgen 0.2.126",
        "cargo_binstall": bool(
            re.search(r"(?:^|\s)1\.21\.0(?:\s|$)", tools.get("cargo_binstall", ""))
        ),
        "espup": bool(re.search(r"(?:^|\s)0\.17\.1(?:\s|$)", tools.get("espup", ""))),
        "esp_rustc": tools.get("esp_rustc", "").startswith("rustc 1.95.0"),
        "xtensa_gcc": tools.get("xtensa_gcc")
        == "xtensa-esp-elf-gcc (crosstool-NG esp-15.2.0_20250920) 15.2.0",
        "llvm_objcopy": "llvm" in tools.get("llvm_objcopy", "").lower(),
    }
    failed = [name for name, passed in checks.items() if not passed]
    if failed:
        raise ValueError(f"release production tool versions disagree with exact pins: {failed}")


def build_metadata(
    *,
    commit: str,
    source_date_epoch: int,
    tools: dict[str, str],
    root: Path = ROOT,
    system: str | None = None,
    machine: str | None = None,
) -> dict:
    if len(commit) != 40 or any(character not in "0123456789abcdef" for character in commit):
        raise ValueError("build metadata commit must be a lowercase full Git commit")
    if source_date_epoch <= 0:
        raise ValueError("SOURCE_DATE_EPOCH must be a positive Unix timestamp")
    validate_tools(tools)
    timestamp = datetime.fromtimestamp(source_date_epoch, timezone.utc).replace(microsecond=0)
    return {
        "schema": 2,
        "source_commit": commit,
        "source_date_epoch": source_date_epoch,
        "built_at_utc": timestamp.isoformat(),
        "timestamp_source": "source_commit",
        "host": {
            "system": system if system is not None else platform.system(),
            "machine": machine if machine is not None else platform.machine(),
        },
        "expected_tools": EXPECTED_TOOLS,
        "tools": tools,
        "web_packages": exact_web_packages(root),
    }


def validate_metadata(metadata: dict, *, commit: str) -> None:
    if set(metadata) != {
        "schema",
        "source_commit",
        "source_date_epoch",
        "built_at_utc",
        "timestamp_source",
        "host",
        "expected_tools",
        "tools",
        "web_packages",
    }:
        raise ValueError("candidate build metadata has an unsupported shape")
    if metadata.get("schema") != 2 or metadata.get("source_commit") != commit:
        raise ValueError("candidate build metadata disagrees with its source commit")
    epoch = metadata.get("source_date_epoch")
    if not isinstance(epoch, int) or isinstance(epoch, bool) or epoch <= 0:
        raise ValueError("candidate build metadata has an invalid source epoch")
    expected_timestamp = datetime.fromtimestamp(epoch, timezone.utc).replace(microsecond=0).isoformat()
    if metadata.get("built_at_utc") != expected_timestamp or metadata.get("timestamp_source") != "source_commit":
        raise ValueError("candidate build timestamp is not deterministically source-derived")
    if metadata.get("expected_tools") != EXPECTED_TOOLS:
        raise ValueError("candidate expected production tools drifted")
    tools = metadata.get("tools")
    if not isinstance(tools, dict):
        raise ValueError("candidate resolved production tools are missing")
    validate_tools(tools)
    if metadata.get("web_packages") != EXPECTED_WEB_PACKAGES:
        raise ValueError("candidate exact web package pins drifted")
    host = metadata.get("host")
    if not isinstance(host, dict) or set(host) != {"system", "machine"}:
        raise ValueError("candidate build host identity is malformed")
