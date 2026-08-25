from validation.interop.host_contract import (
    ROOT,
    build_host_library,
    dynamic_library_path,
    environment,
    host_contract_main,
    run_command,
)


SUCCESS = "HOST_PYTHON_CONTRACT_SMOKE_OK"


def run() -> None:
    build_host_library()
    run_command(
        ("python3", ROOT / "prns-host/bindings/python/tests/smoke.py"),
        "Python host contract smoke failed",
        command_environment=environment(
            {
                "PYTHONPATH": ROOT / "prns-host/bindings/python/src",
                "PRNS_HOST_LIBRARY": dynamic_library_path(),
            }
        ),
    )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
