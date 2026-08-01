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
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--package-dir")
    source.add_argument("--public", action="store_true")
    args = parser.parse_args()
    version = (ROOT / "VERSION").read_text().strip()
    package_dir = None
    if args.package_dir is not None:
        package_dir = Path(args.package_dir).resolve()
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
            / "persistent-two-node-v1.json",
            conformance / "persistent-two-node-v1.json",
        )
        environment = os.environ.copy()
        environment["DOTNET_CLI_HOME"] = str(consumer / ".dotnet")
        environment["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1"
        environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1"
        restore = ["dotnet", "restore", str(project), "--source"]
        restore.append(
            str(package_dir)
            if package_dir is not None
            else "https://api.nuget.org/v3/index.json"
        )
        subprocess.run(
            restore,
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
