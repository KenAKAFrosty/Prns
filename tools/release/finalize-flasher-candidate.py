#!/usr/bin/env python3
"""Finalize an unsigned flasher candidate without handling private signing keys."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import sys

from flasher_sparse_sizes import SHIPPING_BOARDS
from flasher_sparse_sizes import build_report as build_sparse_size_report
from flasher_sparse_sizes import render_summary as render_sparse_size_summary
from flasher_hotfix import resolve_release_identity, verify_candidate as verify_hotfix_candidate
from flasher_website_history import allowed_historical_signatures
from flasher_manifest import FLASH_MANIFEST_SCHEMA


ROOT = Path(__file__).resolve().parents[2]
TARGETS = (
    ("aarch64-apple-darwin", ".tar.gz"),
    ("x86_64-apple-darwin", ".tar.gz"),
    ("x86_64-unknown-linux-gnu", ".tar.gz"),
    ("aarch64-unknown-linux-gnu", ".tar.gz"),
    ("x86_64-pc-windows-msvc", ".zip"),
)
CHANNELS = {"stable", "preview"}
EXCLUDED_FROM_SUMS = {"SHA256SUMS.txt", "acceptance.json"}


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def read_version(candidate: Path) -> str:
    candidate_version = (candidate / "VERSION").read_text(encoding="utf-8").strip()
    expected, _ = resolve_release_identity(ROOT, candidate_version)
    if candidate_version != expected or not candidate_version or candidate_version.lower() == "next":
        raise ValueError("candidate VERSION has no publishable repository release identity")
    if any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+" for character in candidate_version):
        raise ValueError("candidate VERSION is not path-safe")
    return candidate_version


def require_unsigned(candidate: Path) -> None:
    allowed = allowed_historical_signatures(candidate)
    signatures = {
        path.relative_to(candidate).as_posix() for path in candidate.rglob("*.minisig")
    }
    unexpected = sorted(signatures - allowed)
    if unexpected:
        raise ValueError(
            f"candidate already contains current or untracked signatures: {unexpected}"
        )
    if (candidate / "SHA256SUMS.txt").exists():
        raise ValueError("candidate is already finalized; use a fresh unsigned directory")


def validate_manifest(candidate: Path, version: str, channel: str, commit: str, key_id: str) -> dict:
    manifest_path = candidate / "flash-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    release = manifest.get("release", {})
    signing = manifest.get("signing", {})
    if manifest.get("schema") != FLASH_MANIFEST_SCHEMA:
        raise ValueError(f"candidate manifest is not schema {FLASH_MANIFEST_SCHEMA}")
    if release != {"version": version, "channel": channel, "commit": commit}:
        raise ValueError("candidate manifest release identity disagrees with finalization inputs")
    if signing != {"key_id": key_id}:
        raise ValueError("candidate manifest signing key ID disagrees with finalization input")
    public_key_lines = (candidate / "minisign.pub").read_text(encoding="utf-8").splitlines()
    prefix = "untrusted comment: minisign public key "
    if not public_key_lines:
        raise ValueError("candidate Minisign public key is empty")
    public_key_line = public_key_lines[0]
    pinned_key_id = public_key_line.removeprefix(prefix).strip()
    if not public_key_line.startswith(prefix) or pinned_key_id.upper() != key_id.upper():
        raise ValueError("candidate signing key ID disagrees with its pinned Minisign public key")
    targets = manifest.get("targets", [])
    boards = {
        target.get("board_slug")
        for target in targets
        if isinstance(target, dict) and isinstance(target.get("board_slug"), str)
    }
    if len(targets) != len(SHIPPING_BOARDS) or boards != SHIPPING_BOARDS:
        raise ValueError("candidate manifest does not contain exactly the shipping board set")
    return manifest


def require_cli_archives(candidate: Path, version: str) -> dict[str, tuple[Path, str]]:
    archives: dict[str, tuple[Path, str]] = {}
    for target, extension in TARGETS:
        name = f"hopspot-flash-{version}-{target}{extension}"
        path = candidate / "cli" / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"missing CLI archive cli/{name}")
        archives[target] = (path, digest(path))
    return archives


def render_installers(candidate: Path, version: str, archives: dict[str, tuple[Path, str]]) -> None:
    shell_hashes = "\n".join(
        f'  {target}) archive="{path.name}"; expected="{checksum}" ;;'
        for target, (path, checksum) in archives.items()
        if not target.endswith("windows-msvc")
    )
    shell = f"""#!/bin/sh
set -eu

version='{version}'
repository='https://github.com/KenAKAFrosty/Prns'
prefix="${{HOPSPOT_FLASH_INSTALL_DIR:-${{XDG_BIN_HOME:-$HOME/.local/bin}}}}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target='aarch64-apple-darwin' ;;
  Darwin-x86_64) target='x86_64-apple-darwin' ;;
  Linux-x86_64) target='x86_64-unknown-linux-gnu' ;;
  Linux-aarch64|Linux-arm64) target='aarch64-unknown-linux-gnu' ;;
  *) echo 'Unsupported operating system or architecture; use the manual downloads.' >&2; exit 2 ;;
esac
case "$target" in
{shell_hashes}
  *) echo 'Installer target table is incomplete.' >&2; exit 2 ;;
esac

temporary="$(mktemp -d "${{TMPDIR:-/tmp}}/hopspot-flash.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
url="$repository/releases/download/v$version/$archive"
curl --fail --location --proto '=https' --tlsv1.2 --output "$temporary/$archive" "$url"
if command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$temporary/$archive" | awk '{{print $1}}')"
elif command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$temporary/$archive" | awk '{{print $1}}')"
else
  echo 'A SHA-256 utility (shasum or sha256sum) is required.' >&2
  exit 2
fi
if [ "$actual" != "$expected" ]; then
  echo "SHA-256 mismatch for $archive" >&2
  exit 4
fi
tar -xzf "$temporary/$archive" -C "$temporary"
mkdir -p "$prefix"
install -m 0755 "$temporary/hopspot-flash" "$prefix/hopspot-flash"
echo "Installed verified hopspot-flash $version to $prefix/hopspot-flash"
"""
    windows_path, windows_hash = archives["x86_64-pc-windows-msvc"]
    powershell = f"""$ErrorActionPreference = 'Stop'
$Version = '{version}'
$Archive = '{windows_path.name}'
$Expected = '{windows_hash}'
$Repository = 'https://github.com/KenAKAFrosty/Prns'
$Prefix = if ($env:HOPSPOT_FLASH_INSTALL_DIR) {{ $env:HOPSPOT_FLASH_INSTALL_DIR }} else {{ Join-Path $env:LOCALAPPDATA 'Programs\\Prns\\bin' }}
$Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $Temporary | Out-Null
try {{
    $Download = Join-Path $Temporary $Archive
    Invoke-WebRequest -Uri "$Repository/releases/download/v$Version/$Archive" -OutFile $Download
    $Actual = (Get-FileHash -Algorithm SHA256 $Download).Hash.ToLowerInvariant()
    if ($Actual -ne $Expected) {{ throw "SHA-256 mismatch for $Archive" }}
    Expand-Archive -Path $Download -DestinationPath $Temporary -Force
    New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
    Copy-Item (Join-Path $Temporary 'hopspot-flash.exe') (Join-Path $Prefix 'hopspot-flash.exe') -Force
    Write-Host "Installed verified hopspot-flash $Version to $Prefix"
}} finally {{
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}}
"""
    manual = f"""# hopspot-flash {version}

Download the archive matching your operating system and architecture from the immutable
`v{version}` GitHub Release. Download `SHA256SUMS.txt` and `SHA256SUMS.txt.minisig` too.

Verify release custody before installing:

```sh
minisign -Vm SHA256SUMS.txt -p minisign.pub
archive=hopspot-flash-{version}-TARGET.ARCHIVE
expected=$(awk -v wanted="cli/$archive" '$2 == wanted {{ print $1 }}' SHA256SUMS.txt)
printf '%s  %s\n' "$expected" "$archive" | shasum -a 256 -c -
```

The checked `install.sh` and `install.ps1` embed the exact archive SHA-256 and install to a
user-owned directory by default. Neither installer requires administrator access. Apple
notarization and Authenticode are not claimed for this release.
"""
    install_dir = candidate / "cli"
    (install_dir / "install.sh").write_text(shell, encoding="utf-8", newline="\n")
    (install_dir / "install.ps1").write_text(powershell, encoding="utf-8", newline="\n")
    (install_dir / "README.md").write_text(manual, encoding="utf-8", newline="\n")


def stage_website(candidate: Path, version: str, channel: str, descriptor_path: Path) -> None:
    website = candidate / "website"
    if not (website / "index.html").is_file():
        raise ValueError("candidate website/index.html is missing")
    release_dir = website / "releases" / version
    if release_dir.exists():
        raise ValueError(f"website immutable release directory already exists: {release_dir}")
    release_dir.mkdir(parents=True)
    shutil.copy2(candidate / "flash-manifest.json", release_dir / "flash-manifest.json")
    shutil.copytree(candidate / "firmware", release_dir / "firmware")
    channel_dir = website / "releases" / "channels"
    channel_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(descriptor_path, channel_dir / f"{channel}.json")
    shutil.copy2(candidate / "minisign.pub", website / "releases" / "minisign.pub")


def payload_files(candidate: Path) -> list[Path]:
    output = []
    for path in candidate.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate cannot contain symlink {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(candidate).as_posix()
        if relative in EXCLUDED_FROM_SUMS or relative.endswith(".minisig"):
            continue
        pure = PurePosixPath(relative)
        if pure.is_absolute() or ".." in pure.parts:
            raise ValueError(f"unsafe candidate path {relative}")
        output.append(path)
    return sorted(output, key=lambda path: path.relative_to(candidate).as_posix())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--channel", choices=sorted(CHANNELS), required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--key-id", required=True)
    arguments = parser.parse_args()
    candidate = arguments.candidate.resolve()
    try:
        if len(arguments.commit) != 40 or any(character not in "0123456789abcdefABCDEF" for character in arguments.commit):
            raise ValueError("--commit must be a full 40-character Git hash")
        if not arguments.key_id.strip():
            raise ValueError("--key-id is required")
        require_unsigned(candidate)
        version = read_version(candidate)
        manifest = validate_manifest(
            candidate,
            version,
            arguments.channel,
            arguments.commit,
            arguments.key_id,
        )
        verify_hotfix_candidate(ROOT, candidate)
        archives = require_cli_archives(candidate, version)
        render_installers(candidate, version, archives)

        manifest_hash = digest(candidate / "flash-manifest.json")
        descriptor = {
            "schema": 1,
            "channel": arguments.channel,
            "version": version,
            "manifest_url": f"https://reticulum.rs/releases/{version}/flash-manifest.json",
            "manifest_sha256": manifest_hash,
        }
        channel_dir = candidate / "channels"
        channel_dir.mkdir(parents=True, exist_ok=True)
        descriptor_path = channel_dir / f"{arguments.channel}.json"
        descriptor_path.write_text(
            json.dumps(descriptor, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        stage_website(candidate, version, arguments.channel, descriptor_path)

        sparse_report = build_sparse_size_report(manifest)
        sparse_path = candidate / "metadata" / "sparse-sizes.json"
        sparse_path.parent.mkdir(parents=True, exist_ok=True)
        sparse_path.write_text(
            json.dumps(sparse_report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        for line in render_sparse_size_summary(sparse_report):
            print(f"[sparse-size] {line}")

        lines = [
            f"{digest(path)}  {path.relative_to(candidate).as_posix()}"
            for path in payload_files(candidate)
        ]
        (candidate / "SHA256SUMS.txt").write_text(
            "\n".join(lines) + "\n", encoding="utf-8", newline="\n"
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"candidate finalization failed: {error}", file=sys.stderr)
        return 1
    print(f"finalized unsigned flasher candidate {version}; offline signatures are still required")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
