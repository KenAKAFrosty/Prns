#!/usr/bin/env python3

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def cargo_version(path):
    match = re.search(r'^version = "([^"]+)"$', path.read_text(), re.MULTILINE)
    if match is None:
        raise ValueError(f"missing package version in {path}")
    return match.group(1)


def project_version(path):
    match = re.search(r"<Version>([^<]+)</Version>", path.read_text())
    if match is None:
        raise ValueError(f"missing project version in {path}")
    return match.group(1)


def pyproject_version(path):
    match = re.search(
        r'^\[project\][\s\S]*?^version = "([^"]+)"$',
        path.read_text(),
        re.MULTILINE,
    )
    if match is None:
        raise ValueError(f"missing project version in {path}")
    return match.group(1)


def assignment_version(path):
    match = re.search(r'^version\s*=\s*"([^"]+)"$', path.read_text(), re.MULTILINE)
    if match is None:
        raise ValueError(f"missing assigned version in {path}")
    return match.group(1)


def main():
    expected = (ROOT / "VERSION").read_text().strip()
    catalog = json.loads(
        (ROOT / "prns-host/distribution/packages.json").read_text()
    )
    schema = json.loads(
        (ROOT / "prns-host/schema/host-contract-v1.json").read_text()
    )
    versions = {
        "schema": schema["productVersion"],
        "host-core": cargo_version(ROOT / "prns-host/core/Cargo.toml"),
        "host-c": cargo_version(ROOT / "prns-host/abi/c/Cargo.toml"),
        "host-native": cargo_version(
            ROOT / "prns-host/impls/native/Cargo.toml"
        ),
        "dotnet": project_version(
            ROOT
            / "prns-host/bindings/dotnet/src/PersonalRns/PersonalRns.csproj"
        ),
        "python": pyproject_version(
            ROOT / "prns-host/bindings/python/pyproject.toml"
        ),
        "jvm": assignment_version(
            ROOT / "prns-host/bindings/jvm/build.gradle.kts"
        ),
        "julia": assignment_version(
            ROOT / "prns-host/bindings/julia/Project.toml"
        ),
        "npm": json.loads((ROOT / "prns-js/package.json").read_text())[
            "version"
        ],
    }
    versions.update(
        {
            f"rust:{crate['name']}": cargo_version(ROOT / crate["manifest"])
            for crate in catalog["rustCrates"]
        }
    )
    disagreements = {
        name: version for name, version in versions.items() if version != expected
    }
    if disagreements:
        raise SystemExit(
            f"host SDK versions disagree with VERSION={expected}: {disagreements}"
        )


if __name__ == "__main__":
    main()
