from __future__ import annotations

import sys

from validation.hardening import embedded_miri
from validation.hardening.embedded_isa.contract import InventoryError
from validation.hardening.embedded_platform.contract import (
    InventoryError as PlatformInventoryError,
)
from validation.hardening.embedded_readiness.checks import inspect
from validation.hardening.embedded_readiness.contract import load_contract
from validation.hardening.embedded_readiness.error import ReadinessError
from validation.hardening.embedded_readiness.model import (
    ReadinessCheck,
    ReadinessStatus,
)
from validation.hardening.embedded_readiness.system import SystemProbe


def render(checks: tuple[ReadinessCheck, ...]) -> ReadinessStatus:
    status = ReadinessStatus.READY
    for check in checks:
        destination = sys.stdout if check.passed() else sys.stderr
        print(
            f"[embedded-assurance] {check.state.value}: "
            f"lane={check.lane.value} subject={check.subject}; {check.detail}",
            file=destination,
        )
        if not check.passed():
            status = ReadinessStatus.NOT_READY
            for command in check.setup:
                print(f"[embedded-assurance] setup: {command}", file=sys.stderr)
    return status


def main() -> int:
    try:
        status = render(inspect(load_contract(), SystemProbe()))
    except (
        OSError,
        InventoryError,
        PlatformInventoryError,
        ReadinessError,
        embedded_miri.EmbeddedMiriError,
    ) as error:
        print(f"EMBEDDED_ASSURANCE_READINESS_ERROR: {error}", file=sys.stderr)
        return 1
    if status is ReadinessStatus.READY:
        print("EMBEDDED_ASSURANCE_READINESS_OK")
        return 0
    print("EMBEDDED_ASSURANCE_READINESS_FAILED", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
