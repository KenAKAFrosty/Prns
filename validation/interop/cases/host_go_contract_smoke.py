import tempfile
from pathlib import Path

from validation.interop.host_contract import (
    ROOT,
    build_host_library,
    dynamic_library_path,
    environment,
    host_contract_main,
    package_host_native,
    run_command,
)


SUCCESS = "HOST_GO_CONTRACT_SMOKE_OK"
GO_RUST_TARGET = "x86_64-unknown-linux-gnu"


def run() -> None:
    build_host_library()
    with tempfile.TemporaryDirectory(prefix="prns-host-go-") as temporary:
        scratch = Path(temporary)
        native = scratch / "native"
        package_host_native(
            output=native,
            rust_target=GO_RUST_TARGET,
            dynamic_library=dynamic_library_path(),
        )
        run_command(
            (
                "go",
                "-C",
                ROOT / "prns-host/bindings/go",
                "test",
                "-race",
                "./...",
            ),
            "Go host contract smoke failed",
            command_environment=environment(
                {
                    "PKG_CONFIG_PATH": native / "lib/pkgconfig",
                    "LD_LIBRARY_PATH": native / "lib",
                    "GOCACHE": scratch / "go-cache",
                    "GOPATH": scratch / "go-path",
                }
            ),
        )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
