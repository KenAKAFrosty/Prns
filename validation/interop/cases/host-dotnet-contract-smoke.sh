#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"

native_library_path="${root}/prns-host/abi/c/target/debug"
dotnet_cli_home="${root}/prns-host/bindings/dotnet/.dotnet-cli"
nuget_packages="${root}/prns-host/bindings/dotnet/.nuget-packages"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked
env \
    LD_LIBRARY_PATH="$native_library_path" \
    DOTNET_CLI_HOME="$dotnet_cli_home" \
    NUGET_PACKAGES="$nuget_packages" \
    DOTNET_CLI_TELEMETRY_OPTOUT=1 \
    DOTNET_NOLOGO=1 \
    DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1 \
    dotnet run \
        --project prns-host/bindings/dotnet/tests/ContractSmoke/ContractSmoke.csproj \
        --configuration Release \
        --property:TreatWarningsAsErrors=true

echo "HOST_DOTNET_CONTRACT_SMOKE_OK"
