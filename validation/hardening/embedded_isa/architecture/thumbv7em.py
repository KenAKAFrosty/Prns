from pathlib import Path


def command(emulator: Path, executable: Path) -> tuple[str, ...]:
    return (
        str(emulator),
        "-machine",
        "mps2-an386",
        "-cpu",
        "cortex-m4",
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
