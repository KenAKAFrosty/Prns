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


def package_json(path):
    return json.loads(path.read_text())


def main():
    expected = (ROOT / "VERSION").read_text().strip()
    catalog = json.loads(
        (ROOT / "prns-host/distribution/packages.json").read_text()
    )
    schema = json.loads(
        (ROOT / "prns-host/schema/host-contract-v1.json").read_text()
    )
    javascript = package_json(ROOT / "prns-js/package.json")
    hopspot_javascript = package_json(
        ROOT / "personal-hopspot/sdk/hopspot/package.json"
    )
    napi = package_json(ROOT / "prns-napi/package.json")
    wasm = package_json(ROOT / "prns-wasm/package.json")
    expected_npm_packages = {
        target["npmPackage"]
        for target in catalog["nativeTargets"]
        if "npmPackage" in target
    }
    generated_platform_packages = {
        package["name"]: package
        for package in (
            package_json(path)
            for path in sorted((ROOT / "prns-napi/npm").glob("*/package.json"))
        )
    }
    if generated_platform_packages and (
        set(generated_platform_packages) != expected_npm_packages
    ):
        raise SystemExit(
            "N-API platform package inventory differs from packages.json"
        )
    optional_dependencies = javascript.get("optionalDependencies", {})
    if set(optional_dependencies) != expected_npm_packages:
        raise SystemExit(
            "JavaScript optional dependency inventory differs from packages.json"
        )
    versions = {
        "schema": schema["productVersion"],
        "host-core": cargo_version(ROOT / "prns-host/core/Cargo.toml"),
        "host-c": cargo_version(ROOT / "prns-host/abi/c/Cargo.toml"),
        "host-native": cargo_version(
            ROOT / "prns-host/impls/native/Cargo.toml"
        ),
        "host-cooperative": cargo_version(
            ROOT / "prns-host/impls/cooperative/Cargo.toml"
        ),
        "host-tokio": cargo_version(
            ROOT / "prns-host/impls/tokio/Cargo.toml"
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
        "javascript": javascript["version"],
        "javascript:hopspot": hopspot_javascript["version"],
        "napi-cargo": cargo_version(ROOT / "prns-napi/Cargo.toml"),
        "napi-package": napi["version"],
        "wasm-cargo": cargo_version(ROOT / "prns-wasm/Cargo.toml"),
        "wasm-package": wasm["version"],
    }
    versions.update(
        {
            f"rust:{crate['name']}": cargo_version(ROOT / crate["manifest"])
            for crate in catalog["rustCrates"]
        }
    )
    versions.update(
        {
            f"napi:{name}": napi["version"]
            for name in expected_npm_packages
        }
    )
    versions.update(
        {
            f"generated-napi:{name}": package["version"]
            for name, package in generated_platform_packages.items()
        }
    )
    versions.update(
        {
            f"javascript-optional:{name}": version
            for name, version in optional_dependencies.items()
        }
    )
    binding_versions = set(
        re.findall(
            r"bindingPackageVersion !== '([^']+)'",
            (ROOT / "prns-napi/index.js").read_text(),
        )
    )
    if binding_versions != {expected}:
        raise SystemExit(
            "generated N-API loader versions disagree with VERSION="
            f"{expected}: {sorted(binding_versions)}"
        )
    if hopspot_javascript.get("dependencies") != {"personal-rns": expected}:
        raise SystemExit("hopspot npm dependency disagrees with VERSION")
    disagreements = {
        name: version for name, version in versions.items() if version != expected
    }
    if disagreements:
        raise SystemExit(
            f"host SDK versions disagree with VERSION={expected}: {disagreements}"
        )


if __name__ == "__main__":
    main()
