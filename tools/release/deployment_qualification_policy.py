"""Version-bound release-owner deferral, distinct from deployment evidence."""

import json
from pathlib import Path


DEFERRED_VERSION = "0.3.8"


def qualification_policy(root: Path, version: str) -> dict:
    if version != DEFERRED_VERSION:
        return {"status": "required"}
    path = root / "release" / "deployment" / f"{version}.json"
    if path.is_symlink():
        raise ValueError("deployment policy must be a regular committed file")
    value = json.loads(path.read_text(encoding="utf-8"))
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "version", "status", "release_owner", "reason"}
        or type(value["schema"]) is not int
        or value["schema"] != 1
        or value["version"] != DEFERRED_VERSION
        or value["status"] != "deferred"
        or value["release_owner"] != "github:KenAKAFrosty"
        or not isinstance(value["reason"], str)
        or not value["reason"].strip()
    ):
        raise ValueError("invalid version-bound deployment qualification deferral")
    return value
