from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import re
import sys
from typing import Iterable


BROWSER_TEST_FIXTURE_MARKER = b"PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1"
LOCAL_DEV_TRUST_MARKER = b"PRNS_LOCAL_DEV_FLASHER_TRUST_ROOT_V1"
LOCAL_DEV_BANNER_MARKER = b"PRNS_LOCAL_DEV_FLASHER_BANNER_V1"
LOCAL_DEV_BANNER = (
    "LOCAL DEVELOPER FIRMWARE — EPHEMERALLY SIGNED, NOT A RELEASE".encode("utf-8")
)
MINISIGN_PUBLIC_KEY_PATTERN = re.compile(
    rb"(?<![A-Za-z0-9+/])RWQ[A-Za-z0-9+/]{53}(?![A-Za-z0-9+/])"
)


class BrowserTestTrustMaterial(Enum):
    FIXTURE_MARKER = "browser-test fixture marker"
    FIXTURE_MINISIGN_PUBLIC_KEY = "browser-test Minisign public key"
    LOCAL_DEV_TRUST_MARKER = "local-development trust marker"
    LOCAL_DEV_BANNER_MARKER = "local-development banner marker"
    LOCAL_DEV_BANNER = "local-development banner"
    EPHEMERAL_MINISIGN_PUBLIC_KEY = "non-production Minisign public key"


@dataclass(frozen=True)
class BrowserTestTrustLeak:
    path: Path
    material: BrowserTestTrustMaterial


def minisign_public_key_payload(path: Path) -> bytes:
    lines = path.read_bytes().splitlines()
    if len(lines) < 2 or not lines[1]:
        raise ValueError(f"Minisign public key has no payload: {path}")
    return lines[1]


def find_browser_test_trust_leaks(
    roots: Iterable[Path],
    fixture_key: Path,
    production_key: Path,
    allowed_exact_blob: Path | None = None,
) -> tuple[BrowserTestTrustLeak, ...]:
    fixture_key_payload = minisign_public_key_payload(fixture_key)
    production_key_payload = minisign_public_key_payload(production_key)
    allowed_blob = allowed_exact_blob.read_bytes() if allowed_exact_blob else None
    if allowed_exact_blob and not allowed_blob:
        raise ValueError(f"allowed exact blob is empty: {allowed_exact_blob}")
    leaks = []
    for root in roots:
        if not root.is_dir():
            raise ValueError(f"trust-scan root is not a directory: {root}")
        for path in sorted(root.rglob("*")):
            if not path.is_file():
                continue
            value = path.read_bytes()
            if allowed_blob:
                value = value.replace(allowed_blob, b"")
            if BROWSER_TEST_FIXTURE_MARKER in value:
                leaks.append(
                    BrowserTestTrustLeak(
                        path=path,
                        material=BrowserTestTrustMaterial.FIXTURE_MARKER,
                    )
                )
            if fixture_key_payload in value:
                leaks.append(
                    BrowserTestTrustLeak(
                        path=path,
                        material=BrowserTestTrustMaterial.FIXTURE_MINISIGN_PUBLIC_KEY,
                    )
                )
            if LOCAL_DEV_TRUST_MARKER in value:
                leaks.append(
                    BrowserTestTrustLeak(
                        path=path,
                        material=BrowserTestTrustMaterial.LOCAL_DEV_TRUST_MARKER,
                    )
                )
            if LOCAL_DEV_BANNER_MARKER in value:
                leaks.append(
                    BrowserTestTrustLeak(
                        path=path,
                        material=BrowserTestTrustMaterial.LOCAL_DEV_BANNER_MARKER,
                    )
                )
            if LOCAL_DEV_BANNER in value:
                leaks.append(
                    BrowserTestTrustLeak(
                        path=path,
                        material=BrowserTestTrustMaterial.LOCAL_DEV_BANNER,
                    )
                )
            for public_key_payload in set(MINISIGN_PUBLIC_KEY_PATTERN.findall(value)):
                if public_key_payload not in {
                    production_key_payload,
                    fixture_key_payload,
                }:
                    leaks.append(
                        BrowserTestTrustLeak(
                            path=path,
                            material=BrowserTestTrustMaterial.EPHEMERAL_MINISIGN_PUBLIC_KEY,
                        )
                    )
    return tuple(leaks)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture-key", type=Path, required=True)
    parser.add_argument("--production-key", type=Path, required=True)
    parser.add_argument("--allow-exact-blob", type=Path)
    parser.add_argument("roots", type=Path, nargs="+")
    arguments = parser.parse_args()
    try:
        leaks = find_browser_test_trust_leaks(
            arguments.roots,
            arguments.fixture_key,
            arguments.production_key,
            arguments.allow_exact_blob,
        )
    except (OSError, ValueError) as error:
        print(f"browser-test trust scan failed: {error}", file=sys.stderr)
        return 2
    for leak in leaks:
        print(
            f"a production output contains the {leak.material.value}: {leak.path}",
            file=sys.stderr,
        )
    return 1 if leaks else 0


if __name__ == "__main__":
    raise SystemExit(main())
