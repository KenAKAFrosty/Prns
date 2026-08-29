import tempfile
from pathlib import Path

from validation.interop.host_contract import (
    ROOT,
    HOST_C_TARGET,
    build_host_library,
    environment,
    host_contract_main,
    package_host_native,
    run_command,
    swift_host_target,
)


SUCCESS = "HOST_SWIFT_CONTRACT_SMOKE_OK"


def run() -> None:
    build_host_library()
    target = swift_host_target()
    with tempfile.TemporaryDirectory(prefix="prns-host-swift-") as temporary:
        scratch = Path(temporary)
        native = scratch / "native"
        package_host_native(
            output=native,
            rust_target=target.rust_target,
            dynamic_library=HOST_C_TARGET / target.dynamic_library_name,
        )
        run_command(
            (
                "swift",
                "test",
                "--package-path",
                ROOT / "prns-host/bindings/swift",
                "--scratch-path",
                scratch / "build",
            ),
            "Swift host contract smoke failed",
            command_environment=environment(
                {
                    "PKG_CONFIG_PATH": native / "lib/pkgconfig",
                    "LD_LIBRARY_PATH": native / "lib",
                    "DYLD_LIBRARY_PATH": native / "lib",
                    "CLANG_MODULE_CACHE_PATH": scratch / "clang-cache",
                    "XDG_CONFIG_HOME": scratch / "config",
                    "XDG_CACHE_HOME": scratch / "cache",
                }
            ),
        )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
