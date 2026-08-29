import tempfile
from pathlib import Path

from validation.interop.host_contract import (
    ROOT,
    HOST_C_TARGET,
    build_host_library,
    dynamic_library_path,
    environment,
    host_contract_main,
    run_command,
)


SUCCESS = "HOST_JVM_CONTRACT_SMOKE_OK"


def run() -> None:
    build_host_library()
    binding = ROOT / "prns-host/bindings/jvm"
    with tempfile.TemporaryDirectory(prefix="prns-host-jvm-") as temporary:
        run_command(
            (
                binding / "gradlew",
                "--project-dir",
                binding,
                "test",
                "--no-daemon",
                "--non-interactive",
                f"-Dpersonal.rns.library={dynamic_library_path()}",
            ),
            "JVM host contract smoke failed",
            command_environment=environment(
                {
                    "GRADLE_USER_HOME": Path(temporary) / "gradle",
                    "LD_LIBRARY_PATH": HOST_C_TARGET,
                }
            ),
        )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
