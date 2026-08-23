from validation.interop.host_contract import (
    ROOT,
    HOST_C_TARGET,
    build_host_library,
    environment,
    host_contract_main,
    run_command,
)


SUCCESS = "HOST_DOTNET_CONTRACT_SMOKE_OK"


def run() -> None:
    build_host_library()
    binding = ROOT / "prns-host/bindings/dotnet"
    run_command(
        (
            "dotnet",
            "run",
            "--project",
            binding / "tests/ContractSmoke/ContractSmoke.csproj",
            "--configuration",
            "Release",
            "--property:TreatWarningsAsErrors=true",
        ),
        ".NET host contract smoke failed",
        command_environment=environment(
            {
                "LD_LIBRARY_PATH": HOST_C_TARGET,
                "DOTNET_CLI_HOME": binding / ".dotnet-cli",
                "NUGET_PACKAGES": binding / ".nuget-packages",
                "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
                "DOTNET_NOLOGO": "1",
                "DOTNET_SKIP_FIRST_TIME_EXPERIENCE": "1",
            }
        ),
    )


if __name__ == "__main__":
    raise SystemExit(host_contract_main(run, SUCCESS))
