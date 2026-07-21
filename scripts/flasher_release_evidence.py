"""Shared release-evidence primitives for flasher custody scripts."""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def decode_attestation_payload(value: str) -> dict:
    padding = "=" * (-len(value) % 4)
    try:
        decoded = base64.b64decode(value + padding, altchars=b"-_", validate=True)
        payload = json.loads(decoded)
    except (ValueError, json.JSONDecodeError) as error:
        raise ValueError("attestation DSSE payload is malformed") from error
    if not isinstance(payload, dict):
        raise ValueError("attestation statement must be a JSON object")
    return payload


def attestation_subjects(bundle: dict) -> list[dict[str, str]]:
    envelope = bundle.get("dsseEnvelope")
    if not isinstance(envelope, dict) and isinstance(bundle.get("content"), dict):
        envelope = bundle["content"].get("dsseEnvelope")
    if not isinstance(envelope, dict) or not isinstance(envelope.get("payload"), str):
        raise ValueError("attestation bundle has no DSSE envelope")
    statement = decode_attestation_payload(envelope["payload"])
    if statement.get("_type") != "https://in-toto.io/Statement/v1":
        raise ValueError("attestation is not an in-toto Statement v1")
    subjects = statement.get("subject")
    if not isinstance(subjects, list) or not subjects:
        raise ValueError("attestation has no subjects")
    output = []
    names = set()
    for index, subject in enumerate(subjects):
        if not isinstance(subject, dict) or not isinstance(subject.get("digest"), dict):
            raise ValueError(f"attestation subject {index} is malformed")
        name = subject.get("name")
        checksum = subject["digest"].get("sha256")
        if not isinstance(name, str) or not name or not isinstance(checksum, str):
            raise ValueError(f"attestation subject {index} is malformed")
        if len(checksum) != 64 or any(
            character not in "0123456789abcdef" for character in checksum
        ):
            raise ValueError(f"attestation subject {index} has invalid SHA-256")
        if name in names:
            raise ValueError(f"attestation subject name is duplicated: {name}")
        names.add(name)
        output.append({"name": name, "sha256": checksum})
    return sorted(output, key=lambda subject: (subject["name"], subject["sha256"]))
