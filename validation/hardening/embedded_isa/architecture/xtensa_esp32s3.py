import json
from pathlib import Path


def command(emulator: Path, executable: Path) -> tuple[str, ...]:
    return (
        str(emulator),
        "-machine",
        "esp32s3",
        "-cpu",
        "esp32s3",
        "-nographic",
        "-monitor",
        "none",
        "-serial",
        "none",
        "-semihosting-config",
        "enable=on,target=native",
        "-kernel",
        str(executable),
    )


def cargo_arguments(linker: Path, rust_target: str) -> tuple[str, ...]:
    rustflags = (
        '["-C", "default-linker-libraries=yes", '
        '"-C", "link-arg=-specs=sim.elf.specs", '
        '"-C", "link-arg=-specs=sys.qemu.specs"]'
    )
    return (
        "-Zbuild-std=core,alloc",
        "--config",
        f"target.{rust_target}.linker={json.dumps(str(linker))}",
        "--config",
        f"target.{rust_target}.rustflags={rustflags}",
    )
