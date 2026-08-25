import tempfile
from pathlib import Path

from validation.interop.host_contract import (
    ROOT,
    build_host_library,
    dynamic_library_path,
    environment,
    host_contract_main,
    run_command,
)


SUCCESS = "HOST_JULIA_CONTRACT_SMOKE_OK"


def run() -> None:
    build_host_library()
    binding = ROOT / "prns-host/bindings/julia"
    with tempfile.TemporaryDirectory(prefix="prns-host-julia-") as temporary:
        for threads in (1, 2):
            run_command(
                (
                    "julia",
                    f"--project={binding}",
                    f"--threads={threads}",
                    "-e",
                    'using PersonalRns; include("prns-host/bindings/julia/test/runtests.jl")',
                ),
                f"Julia host contract smoke failed with {threads} thread(s)",
                command_environment=environment(
                    {
                        "PRNS_HOST_LIBRARY": dynamic_library_path(),
                        "JULIA_DEPOT_PATH": Path(temporary) / "depot",
                    }
                ),
            )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
