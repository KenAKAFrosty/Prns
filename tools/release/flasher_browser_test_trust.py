from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import sys
from typing import Iterable


BROWSER_TEST_FIXTURE_MARKER = b"PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1"


class BrowserTestTrustMaterial(Enum):
    FIXTURE_MARKER = "browser-test fixture marker"
    MINISIGN_PUBLIC_KEY = "browser-test Minisign public key"


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
    allowed_exact_blob: Path | None = None,
) -> tuple[BrowserTestTrustLeak, ...]:
    fixture_key_payload = minisign_public_key_payload(fixture_key)
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
                        material=BrowserTestTrustMaterial.MINISIGN_PUBLIC_KEY,
                    )
                )
    return tuple(leaks)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture-key", type=Path, required=True)
    parser.add_argument("--allow-exact-blob", type=Path)
    parser.add_argument("roots", type=Path, nargs="+")
    arguments = parser.parse_args()
    try:
        leaks = find_browser_test_trust_leaks(
            arguments.roots,
            arguments.fixture_key,
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
