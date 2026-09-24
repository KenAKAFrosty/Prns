"""Verify licenses and provenance, including children inside hashed npm archives."""

import base64
import binascii


def has_registry_integrity(metadata: dict) -> bool:
    resolved = metadata.get("resolved", "")
    integrity = metadata.get("integrity", "")
    if (
        not isinstance(resolved, str)
        or not resolved.startswith("https://registry.npmjs.org/")
        or not isinstance(integrity, str)
        or not integrity.startswith("sha512-")
    ):
        return False
    try:
        return len(base64.b64decode(integrity.removeprefix("sha512-"), validate=True)) == 64
    except (ValueError, binascii.Error):
        return False


def verify_development_packages(packages: dict, allowed_licenses: set[str]) -> None:
    for path, metadata in packages.items():
        if not path or metadata.get("dev") is not True:
            continue
        if metadata.get("license") not in allowed_licenses:
            raise ValueError(f"website test/build dependency lacks a reviewed license: {path}")
        archive = metadata
        if metadata.get("inBundle") is True:
            parent_path, separator, name = path.rpartition("/node_modules/")
            parent = packages.get(parent_path, {})
            bundled = parent.get("bundleDependencies")
            if (
                not separator
                or parent.get("dev") is not True
                or parent.get("inBundle") is True
                or not isinstance(bundled, list)
                or name not in bundled
            ):
                raise ValueError(f"bundled dependency lacks its declared development archive: {path}")
            archive = parent
        if not has_registry_integrity(archive):
            raise ValueError(f"website test/build dependency lacks registry source/integrity: {path}")
