from pathlib import Path

from validation.hardening.embedded_platform.contract import Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError
from validation.hardening.embedded_platform.platform import esp32s3, nrf52840


def renode_command(
    platform: Platform,
    emulator: Path,
    executable: Path,
    description: Path,
    script: Path,
    config: Path,
) -> tuple[str, ...]:
    if platform.identifier != "nrf52840":
        raise EmbeddedPlatformError(
            f"no Renode platform assurance adapter for {platform.identifier!r}"
        )
    script.write_text(
        nrf52840.script(platform, executable, description), encoding="utf-8"
    )
    config.write_text(nrf52840.config(config.parent), encoding="utf-8")
    return nrf52840.command(emulator, script, config)


def qemu_command(
    platform: Platform, emulator: Path, flash_image: Path
) -> tuple[str, ...]:
    if platform.identifier != "esp32s3":
        raise EmbeddedPlatformError(
            f"no QEMU platform assurance adapter for {platform.identifier!r}"
        )
    return esp32s3.command(platform, emulator, flash_image)
