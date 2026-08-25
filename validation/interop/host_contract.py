from __future__ import annotations

import os
import platform
import subprocess
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Mapping, Sequence


ROOT = Path(__file__).resolve().parents[2]
HOST_C_MANIFEST = ROOT / "prns-host/abi/c/Cargo.toml"
HOST_C_TARGET = ROOT / "prns-host/abi/c/target/debug"
HOST_C_STATIC_LIBRARY = HOST_C_TARGET / "libprns_host.a"
PACKAGE_HOST_NATIVE = ROOT / "tools/release/package-host-native.py"


class HostContractFailureKind(Enum):
    COMMAND_FAILED = "command failed"
    UNSUPPORTED_PLATFORM = "unsupported platform"


class HostContractFailure(RuntimeError):
    def __init__(self, kind: HostContractFailureKind, detail: str):
        self.kind = kind
        self.detail = detail
        super().__init__(f"{kind.value}: {detail}")


@dataclass(frozen=True)
class HostNativeTarget:
    rust_target: str
    dynamic_library_name: str


SWIFT_HOST_TARGETS = {
    ("Darwin", "arm64"): HostNativeTarget(
        rust_target="aarch64-apple-darwin",
        dynamic_library_name="libprns_host.dylib",
    ),
    ("Darwin", "x86_64"): HostNativeTarget(
        rust_target="x86_64-apple-darwin",
        dynamic_library_name="libprns_host.dylib",
    ),
    ("Linux", "aarch64"): HostNativeTarget(
        rust_target="aarch64-unknown-linux-gnu",
        dynamic_library_name="libprns_host.so",
    ),
    ("Linux", "x86_64"): HostNativeTarget(
        rust_target="x86_64-unknown-linux-gnu",
        dynamic_library_name="libprns_host.so",
    ),
}


def environment(values: Mapping[str, object]) -> dict[str, str]:
    configured = os.environ.copy()
    configured.update({name: str(value) for name, value in values.items()})
    return configured


def run_command(
    command: Sequence[str | Path],
    failure: str,
    command_environment: Mapping[str, str] | None = None,
    working_directory: Path = ROOT,
) -> None:
    rendered = tuple(str(part) for part in command)
    try:
        result = subprocess.run(
            rendered,
            cwd=working_directory,
            env=command_environment,
            check=False,
        )
    except OSError as error:
        raise HostContractFailure(
            HostContractFailureKind.COMMAND_FAILED,
            f"{failure}: {error}",
        ) from error
    if result.returncode != 0:
        raise HostContractFailure(
            HostContractFailureKind.COMMAND_FAILED,
            f"{failure}: status {result.returncode}",
        )


def build_host_library() -> None:
    run_command(
        (
            "cargo",
            "build",
            "--manifest-path",
            HOST_C_MANIFEST,
            "--locked",
        ),
        "Prns host C ABI did not build",
    )


def dynamic_library_name() -> str:
    if sys.platform == "darwin":
        return "libprns_host.dylib"
    if os.name == "nt":
        return "prns_host.dll"
    return "libprns_host.so"


def dynamic_library_path() -> Path:
    return HOST_C_TARGET / dynamic_library_name()


def dynamic_loader_variable() -> str:
    if sys.platform == "darwin":
        return "DYLD_LIBRARY_PATH"
    return "LD_LIBRARY_PATH"


def swift_host_target() -> HostNativeTarget:
    host = (platform.system(), platform.machine())
    try:
        return SWIFT_HOST_TARGETS[host]
    except KeyError as error:
        raise HostContractFailure(
            HostContractFailureKind.UNSUPPORTED_PLATFORM,
            f"Swift contract host {host[0]}-{host[1]}",
        ) from error


def package_host_native(
    output: Path,
    rust_target: str,
    dynamic_library: Path,
) -> None:
    run_command(
        (
            sys.executable,
            PACKAGE_HOST_NATIVE,
            "--target",
            rust_target,
            "--library",
            dynamic_library,
            "--library",
            HOST_C_STATIC_LIBRARY,
            "--output",
            output,
        ),
        f"Prns host native package for {rust_target} failed",
    )


def host_contract_main(run: Callable[[], None], success_message: str) -> int:
    try:
        run()
    except (HostContractFailure, OSError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(success_message)
    return 0
