#!/usr/bin/env python3

import json
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
CATALOG_PATH = ROOT / "prns-host" / "distribution" / "packages.json"
RUST_README = ROOT / "prns-host" / "distribution" / "RUST_PACKAGE.md"
PACKAGE_README = ROOT / "prns-host" / "distribution" / "PACKAGE.md"


def package_tables(document):
    yield document.get("dependencies", {})
    yield document.get("dev-dependencies", {})
    yield document.get("build-dependencies", {})
    for target in document.get("target", {}).values():
        yield target.get("dependencies", {})
        yield target.get("dev-dependencies", {})
        yield target.get("build-dependencies", {})


def check_rust(catalog, version):
    crates = catalog["rustCrates"]
    names = {crate["name"] for crate in crates}
    if len(names) != len(crates):
        raise ValueError("Rust crate names must be unique")
    manifests = {crate["manifest"] for crate in crates}
    if len(manifests) != len(crates):
        raise ValueError("Rust crate manifests must be unique")
    expected_readme = RUST_README.read_text()
    for crate in crates:
        manifest_path = ROOT / crate["manifest"]
        document = tomllib.loads(manifest_path.read_text())
        package = document["package"]
        if package["name"] != crate["name"]:
            raise ValueError(f"{manifest_path} has the wrong package name")
        if package["version"] != version:
            raise ValueError(f"{crate['name']} version differs from VERSION")
        if package.get("publish") is not True:
            raise ValueError(f"{crate['name']} is not explicitly publishable")
        if package.get("readme") != "README.md":
            raise ValueError(f"{crate['name']} does not own README.md")
        if package.get("include") != [
            "src/**",
            "tests/**",
            "!tests/**/__pycache__/**",
            "!tests/**/*.pyc",
            "examples/**",
            "Cargo.toml",
            "README.md",
        ]:
            raise ValueError(f"{crate['name']} has an unbounded package payload")
        if not package.get("documentation", "").startswith("https://docs.rs/"):
            raise ValueError(f"{crate['name']} has no docs.rs URL")
        if not package.get("keywords") or not package.get("categories"):
            raise ValueError(f"{crate['name']} has incomplete registry metadata")
        readme = manifest_path.parent / "README.md"
        if not readme.read_text().startswith(expected_readme):
            raise ValueError(
                f"{readme} does not begin with the canonical Rust README"
            )
        for table in package_tables(document):
            for dependency, specification in table.items():
                if not isinstance(specification, dict) or "path" not in specification:
                    continue
                package_name = specification.get("package", dependency)
                if package_name not in names:
                    continue
                if specification.get("version") != f"={version}":
                    raise ValueError(
                        f"{crate['name']} -> {package_name} lacks exact "
                        "registry version"
                    )


def check_native(catalog):
    targets = catalog["nativeTargets"]
    rust_targets = {target["rust"] for target in targets}
    if len(rust_targets) != len(targets):
        raise ValueError("native Rust targets must be unique")
    npm = [target["npmPackage"] for target in targets if "npmPackage" in target]
    dotnet = [
        target["dotnetRuntime"] for target in targets if "dotnetRuntime" in target
    ]
    python = [
        target["pythonPlatform"] for target in targets if "pythonPlatform" in target
    ]
    android = [target["androidAbi"] for target in targets if "androidAbi" in target]
    julia = [target["julia"] for target in targets if "julia" in target]
    if len(npm) != 8 or len(set(npm)) != 8:
        raise ValueError("native catalog must own eight npm targets")
    if len(dotnet) != 8 or len(set(dotnet)) != 8:
        raise ValueError("native catalog must own eight .NET runtimes")
    if len(python) != 8 or len(set(python)) != 8:
        raise ValueError("native catalog must own eight Python platforms")
    if sorted(android) != ["arm64-v8a", "armeabi-v7a"]:
        raise ValueError("native catalog must own the two Android ABIs")
    if len(julia) != 8:
        raise ValueError("native catalog must own eight Julia platforms")
    for target in targets:
        if target["archive"] not in {"tar.gz", "zip"}:
            raise ValueError(f"unsupported archive for {target['rust']}")
        if not target["dynamicLibrary"] or not target["staticLibrary"]:
            raise ValueError(f"incomplete libraries for {target['rust']}")


def check_packages(catalog):
    packages = catalog["packages"]
    identities = {
        (package["ecosystem"], package["name"]) for package in packages
    }
    if len(identities) != len(packages):
        raise ValueError("ecosystem package identities must be unique")
    for package in packages:
        if not (ROOT / package["manifest"]).is_file():
            raise ValueError(f"missing package manifest {package['manifest']}")
    ecosystems = {package["ecosystem"] for package in packages}
    expected = {
        "c",
        "cpp",
        "go",
        "julia",
        "maven",
        "npm",
        "nuget",
        "pypi",
        "swift",
    }
    if ecosystems != expected:
        raise ValueError("host SDK ecosystem inventory is incomplete")
    for package in packages:
        if package["ecosystem"] in {"c", "cpp", "go", "julia", "swift"}:
            if "tag" not in package:
                raise ValueError(
                    f"{package['ecosystem']} package lacks immutable tag custody"
                )
    if (ROOT / "prns-js" / "PACKAGE.md").read_text() != PACKAGE_README.read_text():
        raise ValueError("prns-js/PACKAGE.md differs from the canonical package README")


def main():
    catalog = json.loads(CATALOG_PATH.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    schema = json.loads(
        (ROOT / catalog["contractSource"]).read_text()
    )
    if catalog["format"] != 1 or catalog["versionSource"] != "VERSION":
        raise ValueError("unsupported host SDK distribution catalog")
    if schema["productVersion"] != version:
        raise ValueError("host schema product version differs from VERSION")
    documentation = catalog["documentation"]
    for url in documentation.values():
        if not url.startswith("https://"):
            raise ValueError("distribution links must use HTTPS")
    check_native(catalog)
    check_rust(catalog, version)
    check_packages(catalog)
    print(
        f"HOST_SDK_DISTRIBUTION_OK version={version} "
        f"targets={len(catalog['nativeTargets'])} "
        f"rust_crates={len(catalog['rustCrates'])} "
        f"packages={len(catalog['packages'])}"
    )


if __name__ == "__main__":
    main()
