#!/usr/bin/env python3
"""Generate and check the deduplicated release notice bundle with cargo-about 0.9.1."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "THIRD_PARTY_NOTICES.md"
ABOUT = ROOT / "about.toml"
GRAPHS = (
    ("engine", "Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("daemon", "prnsd/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("desktop Linux", "personal-hopspot/desktop/Cargo.toml", "x86_64-unknown-linux-gnu"),
    ("desktop macOS", "personal-hopspot/desktop/Cargo.toml", "aarch64-apple-darwin"),
    ("desktop Windows", "personal-hopspot/desktop/Cargo.toml", "x86_64-pc-windows-msvc"),
    ("Android", "personal-hopspot/mobile/android/rust/Cargo.toml", "aarch64-linux-android"),
    ("iOS", "personal-hopspot/mobile/ios/rust/Cargo.toml", "aarch64-apple-ios"),
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
        "ESP32-S3 T-Beam",
        "personal-hopspot/embedded/esp32/boards/t-beam-supreme/Cargo.toml",
        "xtensa-esp32s3-none-elf",
    ),
    ("WASM", "prns-wasm/Cargo.toml", "wasm32-unknown-unknown"),
)
MAVEN = (
    ("androidx.annotation:annotation:1.5.0", "Apache-2.0"),
    ("com.github.mik3y:usb-serial-for-android:3.7.0", "MIT"),
    ("org.jetbrains:annotations:13.0", "Apache-2.0"),
    ("org.jetbrains.kotlin:kotlin-stdlib:2.0.20", "Apache-2.0"),
)


def about_version() -> str:
    process = subprocess.run(["cargo", "about", "--version"], text=True, capture_output=True)
    version = process.stdout.strip()
    if process.returncode or version != "cargo-about 0.9.1":
        raise RuntimeError(
            "cargo-about 0.9.1 is required: "
            "cargo install cargo-about --version 0.9.1 --locked --features cli"
        )
    return version


def generate_graph(manifest: str, target: str, directory: Path) -> dict:
    config = directory / f"about-{len(list(directory.iterdir()))}.toml"
    config.write_text(
        ABOUT.read_text(encoding="utf-8").replace(
            "ignore-dev-dependencies = true",
            f'ignore-dev-dependencies = true\ntargets = ["{target}"]',
            1,
        ),
        encoding="utf-8",
    )
    output = config.with_suffix(".json")
    command = [
        "cargo",
        "about",
        "generate",
        "--locked",
        "--fail",
        "--format",
        "json",
        "--config",
        str(config),
        "--manifest-path",
        str(ROOT / manifest),
        "--output-file",
        str(output),
    ]
    process = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if process.returncode:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise RuntimeError(f"cargo-about failed for {manifest} ({target})")
    return json.loads(output.read_text(encoding="utf-8"))


def notice_bundle() -> str:
    version = about_version()
    notices: dict[tuple[str, str], dict] = {}
    with tempfile.TemporaryDirectory(prefix="prns-about-") as temp:
        directory = Path(temp)
        for graph, manifest, target in GRAPHS:
            data = generate_graph(manifest, target, directory)
            for license_info in data["licenses"]:
                text = license_info["text"].strip()
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
    nordic = [key for key in notices if key[0] == "LicenseRef-Nordic-SoftDevice"]
    if len(nordic) != 1:
        raise RuntimeError("Nordic SoftDevice notice was not generated exactly once")

    lines = [
        "# Third-Party Notices",
        "",
        "This checked bundle covers the shipped Rust release graphs and Android runtime artifacts.",
        f"It was generated with `{version}` by `scripts/generate-third-party-notices.py`.",
        "Entries are deduplicated by SPDX identifier and exact notice text.",
        "",
        "## Release graphs",
        "",
    ]
    for graph, manifest, target in GRAPHS:
        lines.append(f"- {graph}: `{manifest}` (`{target}`, locked resolution)")
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
        # Preserve license texts byte-for-byte, including CRLF sequences embedded in upstream
        # notices. Path.read_text/write_text use universal-newline translation on some platforms.
        arguments.output.write_bytes(rendered_bytes)
        try:
            shown = arguments.output.relative_to(ROOT)
        except ValueError:
            shown = arguments.output
        print(f"wrote {shown}")
        return 0
    if not arguments.output.exists() or arguments.output.read_bytes() != rendered_bytes:
        print("THIRD_PARTY_NOTICES.md drifted; review and regenerate with --write", file=sys.stderr)
        return 1
    print("THIRD_PARTY_NOTICES.md matches the locked release graphs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
