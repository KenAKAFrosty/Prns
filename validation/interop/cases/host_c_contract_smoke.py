import tempfile
from dataclasses import dataclass
from pathlib import Path

from validation.interop.host_contract import (
    ROOT,
    HOST_C_TARGET,
    build_host_library,
    dynamic_loader_variable,
    environment,
    host_contract_main,
    run_command,
)


SUCCESS = "HOST_C_CONTRACT_SMOKE_OK"


@dataclass(frozen=True)
class CompilerJourney:
    compiler: str
    standard: str
    language_arguments: tuple[str, ...]


COMPILER_JOURNEYS = (
    CompilerJourney(compiler="cc", standard="c11", language_arguments=()),
    CompilerJourney(
        compiler="c++",
        standard="c++17",
        language_arguments=("-x", "c++"),
    ),
)


def run_journey(journey: CompilerJourney, temporary: Path, version: str) -> None:
    name = Path(journey.compiler).name
    executable = temporary / f"journey-{name}"
    state = temporary / f"state-{name}"
    state.mkdir()
    run_command(
        (
            journey.compiler,
            f"-std={journey.standard}",
            *journey.language_arguments,
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Iprns-host/abi/c/include",
            "prns-host/abi/c/tests/persistent-two-node-smoke.c",
            "-Lprns-host/abi/c/target/debug",
            "-lprns_host",
            "-lpthread",
            "-ldl",
            "-lm",
            "-o",
            executable,
        ),
        f"{journey.standard} host contract journey did not compile",
    )
    run_command(
        (
            executable,
            "prns-host/conformance/persistent-two-node-v1.json",
            "prns-host/conformance/interface-configs-v1.json",
            state,
            version,
        ),
        f"{journey.standard} host contract journey failed",
        command_environment=environment({dynamic_loader_variable(): HOST_C_TARGET}),
    )


def run() -> None:
    build_host_library()
    version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    with tempfile.TemporaryDirectory(prefix="prns-host-c-") as temporary:
        for journey in COMPILER_JOURNEYS:
            run_journey(journey, Path(temporary), version)


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
