from pathlib import Path

from validation.hardening.embedded_platform.contract import EmulatorKind, Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError
from validation.hardening.embedded_platform.platform import nrf52840


def command_for(
    platform: Platform,
    emulator: Path,
    executable: Path,
    description: Path,
    script: Path,
    config: Path,
) -> tuple[str, ...]:
    match (platform.identifier, platform.emulator.kind):
        case ("nrf52840", EmulatorKind.RENODE):
            script.write_text(
                nrf52840.script(platform, executable, description), encoding="utf-8"
            )
            config.write_text(nrf52840.config(config.parent), encoding="utf-8")
            return nrf52840.command(emulator, script, config)
        case _:
            raise EmbeddedPlatformError(
                f"no platform assurance adapter for {platform.identifier!r} "
                f"with {platform.emulator.kind.value!r}"
            )
