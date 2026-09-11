from pathlib import Path

from validation.hardening.embedded_platform.contract import Platform
from validation.hardening.embedded_platform.error import EmbeddedPlatformError


RUN_DURATION = "0.01s"
SVD_DIRECTIVE = (
    b"        ApplySVD @https://dl.antmicro.com/projects/renode/svd/NRF52840.svd.gz\n"
)


def command(emulator: Path, script: Path, config: Path) -> tuple[str, ...]:
    return (
        str(emulator),
        "--plain",
        "--disable-gui",
        "--console",
        "--config",
        str(config),
        str(script),
    )


def script(platform: Platform, executable: Path, description: Path) -> str:
    binary = renode_path(executable)
    model = renode_path(description)
    event = milestone_event(platform).decode("ascii").rstrip("\n")
    return f'''$bin=@{binary}

mach create "{platform.identifier}"
machine LoadPlatformDescription @{model}
sysbus LoadELF $bin
set milestone_hook
"""
cpu.Log(LogLevel.Info, "{event}")
"""
cpu AddHook `sysbus GetSymbolAddress "{platform.milestone_symbol}"` $milestone_hook
emulation RunFor "{RUN_DURATION}"
quit
'''


def config(artifact_directory: Path) -> str:
    history = representable_path(artifact_directory / "renode-history")
    return f"""[general]
history-path = {history}
[monitor]
consume-exceptions-from-command = True
break-script-on-exception = True
"""


def milestone_event(platform: Platform) -> bytes:
    return (
        "PRNS_PLATFORM_MILESTONE schema=1 "
        f"platform={platform.identifier} "
        f"profile={platform.memory_profile} "
        f"milestone={platform.milestone.value}\n"
    ).encode("ascii")


def materialize_offline_model(source: Path, target: Path) -> None:
    model = source.read_bytes()
    if model.count(SVD_DIRECTIVE) != 1:
        raise EmbeddedPlatformError(
            "nRF52840 platform model does not contain its expected SVD directive"
        )
    target.write_bytes(model.replace(SVD_DIRECTIVE, b""))


def parse_milestone(output: bytes, platform: Platform) -> bytes:
    expected = milestone_event(platform).rstrip(b"\n")
    matches = tuple(line for line in output.splitlines() if expected in line)
    if len(matches) != 1:
        raise EmbeddedPlatformError(
            f"{platform.identifier} emulation produced {len(matches)} milestone records"
        )
    return expected + b"\n"


def renode_path(path: Path) -> str:
    return representable_path(path.resolve(strict=True))


def representable_path(path: Path) -> str:
    value = path.as_posix()
    if "\n" in value or "\r" in value:
        raise EmbeddedPlatformError(f"path cannot be represented in Renode syntax: {path}")
    return value.replace("\\", "\\\\").replace(" ", "\\ ")
