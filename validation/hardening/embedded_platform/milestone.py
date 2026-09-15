from validation.hardening.embedded_platform.contract import Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


def event(platform: Platform) -> bytes:
    return (
        "PRNS_PLATFORM_MILESTONE schema=1 "
        f"platform={platform.identifier} "
        f"profile={platform.memory_profile} "
        f"milestone={platform.milestone.value}\n"
    ).encode("ascii")


def parse(output: bytes, platform: Platform) -> bytes:
    expected = event(platform).rstrip(b"\n")
    matches = tuple(line for line in output.splitlines() if expected in line)
    if len(matches) != 1:
        raise EmbeddedPlatformError(
            f"{platform.identifier} emulation produced {len(matches)} milestone records"
        )
    return expected + b"\n"
