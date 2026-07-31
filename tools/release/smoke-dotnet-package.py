#!/usr/bin/env python3

import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package-dir", required=True)
    args = parser.parse_args()
    package_dir = Path(args.package_dir).resolve()
    version = (ROOT / "VERSION").read_text().strip()
    packages = sorted(package_dir.glob(f"PersonalRns.{version}.nupkg"))
    if len(packages) != 1:
        raise SystemExit(
            f"expected one PersonalRns {version} package, found {len(packages)}"
        )
    with tempfile.TemporaryDirectory(prefix="prns-dotnet-package-") as temporary:
        consumer = Path(temporary)
        project = consumer / "PackageSmoke.csproj"
        project.write_text(
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n"
            "  <PropertyGroup>\n"
            "    <OutputType>Exe</OutputType>\n"
            "    <TargetFramework>net8.0</TargetFramework>\n"
            "    <ImplicitUsings>enable</ImplicitUsings>\n"
            "    <Nullable>enable</Nullable>\n"
            "  </PropertyGroup>\n"
            "  <ItemGroup>\n"
            f"    <PackageReference Include=\"PersonalRns\" Version=\"{version}\" />\n"
            "  </ItemGroup>\n"
            "</Project>\n"
        )
        shutil.copy2(
            ROOT
            / "prns-host"
            / "bindings"
            / "dotnet"
            / "tests"
            / "ContractSmoke"
            / "Program.cs",
            consumer / "Program.cs",
        )
        conformance = consumer / "prns-host" / "conformance"
        conformance.mkdir(parents=True)
        shutil.copy2(
            ROOT
            / "prns-host"
            / "conformance"
            / "persistent-two-node-v2.json",
            conformance / "persistent-two-node-v2.json",
        )
        environment = os.environ.copy()
        environment["DOTNET_CLI_HOME"] = str(consumer / ".dotnet")
        environment["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1"
        environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1"
        subprocess.run(
            [
                "dotnet",
                "restore",
                str(project),
                "--source",
                str(package_dir),
            ],
            cwd=consumer,
            env=environment,
            check=True,
        )
        subprocess.run(
            ["dotnet", "run", "--project", str(project), "--no-restore"],
            cwd=consumer,
            env=environment,
            check=True,
        )


if __name__ == "__main__":
    main()
