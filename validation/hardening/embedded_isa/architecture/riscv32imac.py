from pathlib import Path


def command(emulator: Path, executable: Path) -> tuple[str, ...]:
    return (
        str(emulator),
        "-machine",
        "virt",
        "-cpu",
        "sifive-e31",
        "-m",
        "128M",
        "-bios",
        "none",
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
