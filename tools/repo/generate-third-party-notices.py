#!/usr/bin/env python3
"""Generate and check the deduplicated release notice bundle with cargo-about 0.9.1."""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "THIRD_PARTY_NOTICES.md"
ABOUT = ROOT / "about.toml"
GRAPHS = (
    ("engine", "Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("daemon Linux", "prnsd/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("daemon macOS", "prnsd/Cargo.toml", "aarch64-apple-darwin"),
    ("daemon Windows", "prnsd/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("desktop Linux", "personal-hopspot/desktop/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("desktop macOS", "personal-hopspot/desktop/Cargo.toml", "aarch64-apple-darwin"),
    ("desktop Windows", "personal-hopspot/desktop/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("Android", "personal-hopspot/mobile/android/rust/Cargo.toml", "aarch64-linux-android"),
    ("iOS", "personal-hopspot/mobile/ios/rust/Cargo.toml", "aarch64-apple-ios"),
    ("Node addon Linux", "prns-napi/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("Node addon macOS", "prns-napi/Cargo.toml", "aarch64-apple-darwin"),
    ("Node addon Windows", "prns-napi/Cargo.toml", "x86_64-pc-windows-msvc"),
    (
        "Host SDK native",
        "prns-host/impls/native/Cargo.toml",
        "x86_64-unknown-linux-gnu",
    ),
    ("nRF52840", "personal-hopspot/embedded/nrf52840/Cargo.toml", "thumbv7em-none-eabihf"),
    (
        "ESP32-C6",
        "personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/Cargo.toml",
        "riscv32imac-unknown-none-elf",
    ),
    (
        "ESP32-S3 Heltec",
        "personal-hopspot/embedded/esp32/boards/heltec-v4/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    (
        "ESP32-S3 Heltec R8",
        "personal-hopspot/embedded/esp32/boards/heltec-v4-r8/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    (
        "ESP32-S3 T-Beam",
        "personal-hopspot/embedded/esp32/boards/t-beam-supreme/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    ("WASM", "prns-wasm/Cargo.toml", "wasm32-unknown-unknown"),
    ("Nordic DFU browser core", "prns-nrf-dfu-wasm/Cargo.toml", "wasm32-unknown-unknown"),
    ("website Rust/WASM", "docs/website/Cargo.toml", "wasm32-unknown-unknown"),
    (
        "flasher macOS arm64",
        "personal-hopspot/flasher/Cargo.toml",
        "aarch64-apple-darwin",
    ),
    (
        "flasher macOS x86_64",
        "personal-hopspot/flasher/Cargo.toml",
        "x86_64-apple-darwin",
    ),
    (
        "flasher Linux x86_64",
        "personal-hopspot/flasher/Cargo.toml",
        "x86_64-unknown-linux-gnu",
    ),
    (
        "flasher Linux arm64",
        "personal-hopspot/flasher/Cargo.toml",
        "aarch64-unknown-linux-gnu",
    ),
    (
        "flasher Windows x86_64",
        "personal-hopspot/flasher/Cargo.toml",
        "x86_64-pc-windows-msvc",
    ),
)
MAVEN = (
    ("androidx.annotation:annotation:1.5.0", "Apache-2.0"),
    ("com.github.mik3y:usb-serial-for-android:3.7.0", "MIT"),
    ("org.jetbrains:annotations:13.0", "Apache-2.0"),
    ("org.jetbrains.kotlin:kotlin-stdlib:2.0.20", "Apache-2.0"),
)
NPM = (
    ("atob-lite 2.0.0", "MIT", "docs/website/node_modules/atob-lite/LICENSE.md"),
    ("esptool-js 0.6.0", "Apache-2.0", "docs/website/node_modules/esptool-js/LICENSE"),
    ("pako 2.2.0", "MIT", "docs/website/node_modules/pako/LICENSE"),
    ("pako 2.2.0", "Zlib", "release/licenses/pako-Zlib.txt"),
    ("spark-md5 3.0.2", "MIT", "release/licenses/spark-md5-MIT.txt"),
    ("tslib 2.8.1", "0BSD", "docs/website/node_modules/tslib/LICENSE.txt"),
)
VENDORED = (
    (
        "nrf-softdevice-s140-v6 0.1.2-prns.1",
        "LicenseRef-Nordic-SoftDevice",
        "personal-hopspot/embedded/nrf52840/vendor/nrf-softdevice/nrf-softdevice-s140-v6/LICENSE-NORDIC",
        ("nRF52840",),
    ),
    (
        "libdbus 1.14.4",
        "AFL-2.1",
        "release/licenses/libdbus-AFL-2.1.txt",
        ("Node addon Linux", "daemon Linux"),
    ),
)


def normalized_notice_text(value: str) -> str:
    """Keep legal words intact while making presentation whitespace reproducible."""
    normalized = value.replace("\r\n", "\n").replace("\r", "\n")
    lines: list[str] = []
    for line in normalized.split("\n"):
        line = line.rstrip()
        if not line and lines and not lines[-1]:
            continue
        lines.append(line)
    return "\n".join(lines).strip()


def about_binary() -> str:
    """Resolve the pinned tool before isolating Cargo's mutable package cache."""
    binary = shutil.which("cargo-about")
    if binary is None:
        raise RuntimeError(
            "cargo-about 0.9.1 is required: "
            "cargo install cargo-about --version 0.9.1 --locked --features cli"
        )
    return binary


def about_version(binary: str) -> str:
    process = subprocess.run([binary, "--version"], text=True, capture_output=True)
    version = process.stdout.strip()
    if process.returncode or version != "cargo-about 0.9.1":
        raise RuntimeError(
            "cargo-about 0.9.1 is required: "
            "cargo install cargo-about --version 0.9.1 --locked --features cli"
        )
    return version


def isolated_cargo_environment(cargo_home: Path) -> dict[str, str]:
    environment = os.environ.copy()
    environment["CARGO_HOME"] = str(cargo_home)
    return environment


def fetch_manifest(manifest: str, cargo_home: Path) -> None:
    command = [
        "cargo",
        "fetch",
        "--locked",
        "--manifest-path",
        str(ROOT / manifest),
    ]
    process = subprocess.run(
        command,
        cwd=ROOT,
        env=isolated_cargo_environment(cargo_home),
        text=True,
        capture_output=True,
    )
    if process.returncode:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise RuntimeError(f"cargo fetch failed for {manifest}")


def generate_graph(
    manifest: str,
    target: str,
    directory: Path,
    cargo_home: Path,
    binary: str,
) -> dict:
    output = directory / f"about-{len(list(directory.iterdir()))}.json"
    command = [
        binary,
        "generate",
        "--locked",
        "--offline",
        "--fail",
        "--format",
        "json",
        "--config",
        str(ABOUT),
        "--manifest-path",
        str(ROOT / manifest),
        "--target",
        target,
        "--output-file",
        str(output),
    ]
    process = subprocess.run(
        command,
        cwd=ROOT,
        env=isolated_cargo_environment(cargo_home),
        text=True,
        capture_output=True,
    )
    if process.returncode:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise RuntimeError(f"cargo-about failed for {manifest} ({target})")
    return json.loads(output.read_text(encoding="utf-8"))


def notice_bundle() -> str:
    binary = about_binary()
    version = about_version(binary)
    notices: dict[tuple[str, str], dict] = {}
    with tempfile.TemporaryDirectory(prefix="prns-about-") as temp:
        temporary = Path(temp)
        cargo_home = temporary / "cargo-home"
        cargo_home.mkdir()
        directory = temporary / "output"
        directory.mkdir()
        fetched_manifests: set[str] = set()
        for graph, manifest, target in GRAPHS:
            if manifest not in fetched_manifests:
                fetch_manifest(manifest, cargo_home)
                fetched_manifests.add(manifest)
            data = generate_graph(manifest, target, directory, cargo_home, binary)
            for license_info in data["licenses"]:
                text = normalized_notice_text(license_info["text"])
                key = (license_info["id"], text)
                notice = notices.setdefault(
                    key,
                    {
                        "name": license_info["name"],
                        "packages": set(),
                        "graphs": set(),
                    },
                )
                notice["graphs"].add(graph)
                for used in license_info.get("used_by", []):
                    package = used["crate"]
                    notice["packages"].add(f'{package["name"]} {package["version"]}')
        for package, identifier, relative in NPM:
            path = ROOT / relative
            if not path.is_file():
                raise RuntimeError(
                    f"npm notice source {relative} is missing; run npm ci in docs/website"
                )
            text = normalized_notice_text(path.read_text(encoding="utf-8"))
            notice = notices.setdefault(
                (identifier, text),
                {"name": identifier, "packages": set(), "graphs": set()},
            )
            notice["packages"].add(package)
            notice["graphs"].add("website JavaScript")
        for package, identifier, relative, graphs in VENDORED:
            path = ROOT / relative
            if not path.is_file():
                raise RuntimeError(f"vendored notice source {relative} is missing")
            text = normalized_notice_text(path.read_text(encoding="utf-8"))
            notice = notices.setdefault(
                (identifier, text),
                {"name": identifier, "packages": set(), "graphs": set()},
            )
            notice["packages"].add(package)
            notice["graphs"].update(graphs)
    nordic = [key for key in notices if key[0] == "LicenseRef-Nordic-SoftDevice"]
    if len(nordic) != 1:
        raise RuntimeError("Nordic SoftDevice notice was not generated exactly once")

    lines = [
        "# Third-Party Notices",
        "",
        "This checked bundle covers the shipped Rust, JavaScript, and Android release graphs.",
        f"It was generated with `{version}` by `./tools/prns repo notices generate`.",
        "Each locked Rust manifest closure is fetched into a fresh isolated Cargo home before "
        "cargo-about reads its target-filtered packaged license material offline.",
        "Entries are deduplicated by SPDX identifier and canonical notice text; line endings, "
        "trailing space, and repeated blank lines are normalized without changing legal words.",
        "",
        "## Release graphs",
        "",
    ]
    for graph, manifest, target in GRAPHS:
        lines.append(f"- {graph}: `{manifest}` (`{target}`, locked resolution)")
    lines.extend(["", "## Website JavaScript runtime", ""])
    lines.extend(
        [
            "- `esptool-js 0.6.0` — `Apache-2.0`",
            "- `spark-md5 3.0.2` — `MIT` alternative selected from `(WTFPL OR MIT)`",
            "- `atob-lite 2.0.0` — `MIT`",
            "- `pako 2.2.0` — `MIT AND Zlib`",
            "- `tslib 2.8.1` — `0BSD`",
        ]
    )
    lines.extend(["", "## Vendored native code", ""])
    lines.extend(
        [
            "- `libdbus 1.14.4` — `AFL-2.1` alternative selected from its "
            "`AFL-2.1 OR GPL-2.0-or-later` dual license; built from the source vendored by "
            "`libdbus-sys` and statically linked into the Linux `personal-rns` Node addon and "
            "full Linux `prnsd` native release.",
        ]
    )
    lines.extend(["", "## Android Maven runtime", ""])
    for coordinate, expression in MAVEN:
        lines.append(f"- `{coordinate}` — `{expression}`")
    lines.extend(
        [
            "",
            "The Maven coordinate list is checked by Gradle's "
            "`verifyReleaseRuntimeDependencies` task. Its MIT and Apache-2.0 terms are reproduced "
            "among the deduplicated license texts below.",
            "",
            "## License texts",
            "",
        ]
    )
    ordered = sorted(
        notices.items(),
        key=lambda item: (item[0][0], hashlib.sha256(item[0][1].encode()).hexdigest()),
    )
    for (identifier, text), notice in ordered:
        digest = hashlib.sha256(text.encode()).hexdigest()[:12]
        lines.extend(
            [
                f"### {identifier} ({digest})",
                "",
                f"License: {notice['name']}",
                "",
                "Used by: " + ", ".join(f"`{package}`" for package in sorted(notice["packages"])),
                "",
                "Release graphs: " + ", ".join(sorted(notice["graphs"])),
                "",
                "```text",
                text,
                "```",
                "",
            ]
        )
    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true", help="replace THIRD_PARTY_NOTICES.md")
    parser.add_argument("--output", type=Path, default=OUTPUT, help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    try:
        rendered = notice_bundle()
    except RuntimeError as error:
        print(f"notice generation failed: {error}", file=sys.stderr)
        return 2
    rendered_bytes = rendered.encode("utf-8")
    if arguments.write:
        # Write deterministic UTF-8/LF output after source notices have been whitespace-normalized.
        arguments.output.write_bytes(rendered_bytes)
        try:
            shown = arguments.output.relative_to(ROOT)
        except ValueError:
            shown = arguments.output
        print(f"wrote {shown}")
        return 0
    if not arguments.output.exists() or arguments.output.read_bytes() != rendered_bytes:
        committed = (
            arguments.output.read_text(encoding="utf-8")
            if arguments.output.exists()
            else ""
        )
        sys.stderr.writelines(
            difflib.unified_diff(
                committed.splitlines(keepends=True),
                rendered.splitlines(keepends=True),
                fromfile=f"{arguments.output} (committed)",
                tofile=f"{arguments.output} (generated)",
            )
        )
        print("THIRD_PARTY_NOTICES.md drifted; review and regenerate with --write", file=sys.stderr)
        return 1
    print("THIRD_PARTY_NOTICES.md matches the locked release graphs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
