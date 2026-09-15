from pathlib import Path

from validation.hardening.embedded_platform.contract import Platform
from validation.hardening.embedded_platform import milestone
from validation.hardening.embedded_platform.contract import RenodeExecution
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
    if not isinstance(platform.execution, RenodeExecution):
        raise EmbeddedPlatformError("nRF52840 platform pilot requires Renode")
    binary = renode_path(executable)
    model = renode_path(description)
    event = milestone.event(platform).decode("ascii").rstrip("\n")
    return f'''$bin=@{binary}

mach create "{platform.identifier}"
machine LoadPlatformDescription @{model}
sysbus LoadELF $bin
set milestone_hook
"""
cpu.Log(LogLevel.Info, "{event}")
"""
cpu AddHook `sysbus GetSymbolAddress "{platform.execution.milestone_symbol}"` $milestone_hook
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


def materialize_offline_model(source: Path, target: Path) -> None:
    model = source.read_bytes()
    if model.count(SVD_DIRECTIVE) != 1:
        raise EmbeddedPlatformError(
            "nRF52840 platform model does not contain its expected SVD directive"
        )
    target.write_bytes(model.replace(SVD_DIRECTIVE, b""))


def renode_path(path: Path) -> str:
    return representable_path(path.resolve(strict=True))


def representable_path(path: Path) -> str:
    value = path.as_posix()
    if "\n" in value or "\r" in value:
        raise EmbeddedPlatformError(f"path cannot be represented in Renode syntax: {path}")
    return value.replace("\\", "\\\\").replace(" ", "\\ ")
