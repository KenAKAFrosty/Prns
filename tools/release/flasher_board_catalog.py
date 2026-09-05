from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path


BOARD_CATALOG_SCHEMA = 4
FLASH_MANIFEST_SCHEMA = 3
BOARD_AVAILABILITIES = frozenset(("shipping", "qualification"))
BOARD_TRANSPORTS = frozenset(("esp-serial", "uf2-mass-storage", "nrf-serial-dfu"))


@dataclass(frozen=True)
class ReleaseBoards:
    catalog: tuple[str, ...]
    shipping: tuple[str, ...]
    esp_serial: tuple[str, ...]


def _release_boards(entries: object, *, catalog: bool) -> ReleaseBoards:
    if not isinstance(entries, list) or not entries:
        raise ValueError("release board source must contain a nonempty target array")
    shipping: list[str] = []
    esp_serial: list[str] = []
    catalog_slugs: list[str] = []
    seen: set[str] = set()
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"release board source target {index} must be an object")
        slug = entry.get("slug" if catalog else "board_slug")
        transport = entry.get("transport")
        availability = entry.get("availability") if catalog else "shipping"
        if not isinstance(slug, str) or not slug:
            raise ValueError(f"release board source target {index} has an invalid slug")
        if slug in seen:
            raise ValueError(f"release board source contains duplicate board {slug!r}")
        seen.add(slug)
        catalog_slugs.append(slug)
        if transport not in BOARD_TRANSPORTS:
            raise ValueError(f"release board source target {slug!r} has an invalid transport")
        if availability not in BOARD_AVAILABILITIES:
            raise ValueError(f"release board source target {slug!r} has an invalid availability")
        if availability != "shipping":
            continue
        shipping.append(slug)
        if transport == "esp-serial":
            esp_serial.append(slug)
    if not shipping:
        raise ValueError("release board source has no shipping targets")
    return ReleaseBoards(tuple(catalog_slugs), tuple(shipping), tuple(esp_serial))


def boards_from_catalog(path: Path) -> ReleaseBoards:
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict) or document.get("schema") != BOARD_CATALOG_SCHEMA:
        raise ValueError(f"unsupported board catalog schema in {path}")
    return _release_boards(document.get("boards"), catalog=True)


def boards_from_manifest(path: Path) -> ReleaseBoards:
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict) or document.get("schema") != FLASH_MANIFEST_SCHEMA:
        raise ValueError(f"unsupported flash manifest schema in {path}")
    return _release_boards(document.get("targets"), catalog=False)


def release_boards(script: Path) -> ReleaseBoards:
    resolved = script.resolve()
    repository_catalog = resolved.parents[2] / "release" / "flash" / "boards.json"
    if repository_catalog.is_file():
        return boards_from_catalog(repository_catalog)
    candidate_manifest = resolved.parents[1] / "flash-manifest.json"
    if candidate_manifest.is_file():
        return boards_from_manifest(candidate_manifest)
    raise ValueError("could not locate a repository board catalog or candidate flash manifest")
