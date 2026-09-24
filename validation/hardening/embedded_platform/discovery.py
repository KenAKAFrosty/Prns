from __future__ import annotations

import hashlib
from pathlib import Path


RENODE_EXECUTABLE_ENV = "PRNS_RENODE"
RENODE_ROOT_ENV = "PRNS_RENODE_ROOT"


def description_candidates(
    executable: Path, configured_root: str | None
) -> tuple[Path, ...]:
    roots = []
    if configured_root:
        roots.append(Path(configured_root))
    roots.extend(
        (
            executable.parent,
            executable.parent.parent,
            executable.parent.parent / "libexec",
        )
    )
    return tuple(dict.fromkeys(roots))


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()
