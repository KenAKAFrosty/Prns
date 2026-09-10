from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass

from validation.hardening.embedded_isa.error import EmbeddedIsaError


TRANSCRIPT_LINE = re.compile(
    rb"^PRNS_ISA_TRANSCRIPT schema=(?P<schema>[0-9]+) "
    rb"scenarios=(?P<scenarios>[0-9]+) bytes=(?P<bytes>[0-9]+) "
    rb"digest=(?P<digest>[0-9a-f]{64}) events=(?P<events>[0-9a-f]+)$",
    re.MULTILINE,
)
TRANSCRIPT_EVENTS = re.compile(
    rb"(?m)^(PRNS_ISA_TRANSCRIPT[^\r\n]* events=)[0-9a-f]+$"
)


@dataclass(frozen=True)
class Transcript:
    schema: int
    completed_scenarios: int
    events: bytes
    digest: str


def parse(output: bytes, expected_scenarios: int) -> Transcript:
    matches = tuple(TRANSCRIPT_LINE.finditer(output))
    if len(matches) != 1:
        raise EmbeddedIsaError(
            f"target-ISA output contains {len(matches)} transcript records"
        )
    fields = matches[0].groupdict()
    schema = int(fields["schema"])
    completed_scenarios = int(fields["scenarios"])
    declared_bytes = int(fields["bytes"])
    digest = fields["digest"].decode("ascii")
    encoded_events = fields["events"].decode("ascii")
    if len(encoded_events) % 2 != 0:
        raise EmbeddedIsaError("target-ISA transcript event encoding has an odd length")
    events = bytes.fromhex(encoded_events)
    if schema != 1:
        raise EmbeddedIsaError(f"target-ISA transcript schema must be 1, found {schema}")
    if completed_scenarios != expected_scenarios:
        raise EmbeddedIsaError(
            "target-ISA transcript completed "
            f"{completed_scenarios} scenarios, expected {expected_scenarios}"
        )
    if declared_bytes != len(events):
        raise EmbeddedIsaError(
            f"target-ISA transcript declares {declared_bytes} bytes, found {len(events)}"
        )
    actual_digest = hashlib.sha256(events).hexdigest()
    if digest != actual_digest:
        raise EmbeddedIsaError(
            f"target-ISA transcript digest is {digest}, computed {actual_digest}"
        )
    return Transcript(
        schema=schema,
        completed_scenarios=completed_scenarios,
        events=events,
        digest=digest,
    )


def require_match(host: Transcript, target: Transcript) -> None:
    if host != target:
        raise EmbeddedIsaError(
            "target-ISA transcript differs from the host reference: "
            f"host={host.digest} target={target.digest}"
        )


def concise(output: bytes) -> bytes:
    return TRANSCRIPT_EVENTS.sub(rb"\1<captured-in-artifact>", output)
