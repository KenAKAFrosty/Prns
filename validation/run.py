#!/usr/bin/env python3
"""Manifest-driven validation control plane for the Prns release surface."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform as host_platform
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - the workspace MSRV has tomllib
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        print(
            "validation failed: Python 3.11+ (or the tomli package) is required",
            file=sys.stderr,
        )
        raise SystemExit(1)


ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = ROOT / "validation" / "manifest.toml"
TRIAGE_PATH = ROOT / "validation" / "mutation" / "triage.toml"
MANIFEST_SCHEMA = 1
EVIDENCE_SCHEMA = 1
MUTATION_TRIAGE_SCHEMA = 1
VALID_TIERS = {"pr", "release", "scheduled"}
VALID_PLATFORMS = {"any", "linux", "macos", "windows", "android-device"}
VALID_TOOLCHAINS = {
    "stable",
    "nightly",
    "kani",
    "python",
    "node",
    "dotnet",
    "go",
    "swift",
    "julia",
    "jvm",
    "esp",
}
NIGHTLY_TOOLCHAIN_ARGUMENT = "+__NIGHTLY_TOOLCHAIN__"
RUNNER_PYTHON_ARGUMENT = "__RUNNER_PYTHON__"
PYTHON_ARGUMENT_PATTERN = re.compile(r"__[A-Z0-9_]*PYTHON[A-Z0-9_]*__")
MUTATION_SHARDING = "round-robin"
MUTATION_TIME_PATTERN = re.compile(
    r"^(?P<prefix>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})"
    r"(?:\.(?P<fraction>\d+))?(?P<zone>Z|[+-]\d{2}:\d{2})$"
)


class ValidationError(RuntimeError):
    pass


def load_toml(path: Path) -> dict:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"cannot load {path.relative_to(ROOT)}: {error}") from error


def load_manifest(path: Path = MANIFEST_PATH) -> dict:
    manifest = load_toml(path)
    if manifest.get("schema") != MANIFEST_SCHEMA:
        raise ValidationError(f"validation manifest schema must be {MANIFEST_SCHEMA}")
    return manifest


def named_toolchain(manifest: dict, name: str) -> str:
    value = manifest.get("toolchains", {}).get(name)
    if name == "nightly" and isinstance(value, str) and re.fullmatch(
        r"nightly-\d{4}-\d{2}-\d{2}", value
    ):
        return value
    raise ValidationError(f"validation toolchain {name!r} is missing or not exact-pinned")


def native_platform() -> str:
    system = host_platform.system().lower()
    return {"darwin": "macos"}.get(system, system)


def git_head() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def tracked_worktree_is_clean() -> bool:
    result = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return not result.stdout.strip()


def validate_expected_sha(expected_sha: str) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", expected_sha):
        raise ValidationError("release evidence requires a lowercase full 40-character commit SHA")


def discover_kani_harnesses() -> dict[str, str]:
    discovered: dict[str, str] = {}
    for source in sorted((ROOT / "prns-core" / "src").rglob("*.rs")):
        lines = source.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if "#[kani::proof]" not in line:
                continue
            for candidate in lines[index + 1 : index + 8]:
                match = re.search(r"\bfn\s+([A-Za-z0-9_]+)\s*\(", candidate)
                if match:
                    name = match.group(1)
                    if name in discovered:
                        raise ValidationError(f"duplicate Kani harness {name}")
                    discovered[name] = source.relative_to(ROOT).as_posix()
                    break
            else:
                raise ValidationError(
                    f"{source.relative_to(ROOT)} has #[kani::proof] without a nearby function"
                )
    return discovered


def discover_fuzz_targets(manifest_path: Path) -> dict[str, str]:
    fuzz = load_toml(manifest_path)
    return {entry["name"]: entry["path"] for entry in fuzz.get("bin", [])}


def expanded_suite(suite: dict) -> list[dict]:
    total = suite.get("shards")
    if isinstance(total, bool) or not isinstance(total, int) or not 2 <= total <= 16:
        return [dict(suite)]
    expanded = []
    for index in range(total):
        shard = dict(suite)
        shard.pop("shards")
        shard["id"] = f"{suite['id']}-shard-{index}-of-{total}"
        shard["artifacts"] = f"{suite['artifacts']}-shard-{index}-of-{total}"
        shard["command"] = [
            *suite["command"],
            "--shard",
            f"{index}/{total}",
            "--sharding",
            MUTATION_SHARDING,
        ]
        shard["shard"] = {
            "suite": suite["id"],
            "index": index,
            "total": total,
        }
        expanded.append(shard)
    return expanded


def virtual_suites(manifest: dict) -> list[dict]:
    suites = [
        expanded
        for suite in manifest.get("suite", [])
        for expanded in expanded_suite(suite)
    ]
    for proof in manifest.get("kani", []):
        name = proof["name"]
        suites.append(
            {
                "id": f"kani-{name}",
                "domain": "kani",
                "group": proof["group"],
                "tiers": proof["tiers"],
                "platform": "any",
                "toolchain": "kani",
                "timeout_seconds": proof.get("timeout_seconds", 900),
                "command": ["cargo", "kani", "-p", "prns-core", "--harness", name],
                "inputs": [proof["source"]],
                "artifacts": f"validation-artifacts/results/kani-{name}",
            }
        )
    for target in manifest.get("fuzz_target", []):
        name = target["name"]
        nightly = named_toolchain(manifest, "nightly")
        suites.append(
            {
                "id": f"fuzz-{name}",
                "domain": "fuzz",
                "group": target["group"],
                "tiers": target["tiers"],
                "platform": "any",
                "toolchain": "nightly",
                "timeout_seconds": target.get("timeout_seconds", 600),
                "command": [
                    "cargo",
                    f"+{nightly}",
                    "fuzz",
                    "run",
                    "--fuzz-dir",
                    ".",
                    name,
                    "__FUZZ_CORPUS__",
                    "--",
                    "-max_total_time=__FUZZ_SECONDS__",
                    "-artifact_prefix=__FUZZ_ARTIFACT_PREFIX__",
                ],
                "inputs": [
                    "validation/fuzz/Cargo.toml",
                    f"validation/fuzz/{target['path']}",
                ],
                "artifacts": f"validation-artifacts/results/fuzz-{name}",
                "working_directory": "validation/fuzz",
            }
        )
    if any(
        NIGHTLY_TOOLCHAIN_ARGUMENT in suite.get("command", [])
        for suite in suites
    ):
        nightly = named_toolchain(manifest, "nightly")
        for suite in suites:
            suite["command"] = [
                f"+{nightly}" if part == NIGHTLY_TOOLCHAIN_ARGUMENT else part
                for part in suite.get("command", [])
            ]
    return suites


def suite_map(manifest: dict) -> dict[str, dict]:
    suites: dict[str, dict] = {}
    for suite in virtual_suites(manifest):
        identifier = suite.get("id")
        if not isinstance(identifier, str) or not identifier:
            raise ValidationError("every suite needs a non-empty id")
        if identifier in suites:
            raise ValidationError(f"duplicate suite id {identifier}")
        suites[identifier] = suite
    return suites


def tracked_or_untracked_sources() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [ROOT / line for line in result.stdout.splitlines() if line and (ROOT / line).is_file()]


def validation_asset_inventory() -> set[str]:
    return {
        path.relative_to(ROOT).as_posix()
        for path in tracked_or_untracked_sources()
        if path.name.endswith("-smoke.sh")
        or (
            path.parent == ROOT / "validation" / "interop" / "cases"
            and path.suffix == ".py"
            and path.name != "__init__.py"
        )
        or path == ROOT / "validation" / "interop" / "harness.py"
        or ("validation/oracles/python" in path.as_posix() and path.suffix == ".py")
        or ("validation/oracles/tests" in path.as_posix() and path.suffix == ".rs")
        or ("validation/interop/peers" in path.as_posix() and path.suffix == ".py")
        or ("prns-wasm/smoke" in path.as_posix() and path.suffix == ".ts")
    }


def source_cargo_manifests() -> set[str]:
    return {
        path.relative_to(ROOT).as_posix()
        for path in tracked_or_untracked_sources()
        if path.name == "Cargo.toml"
    }


def source_cargo_lock_workspaces() -> set[str]:
    workspaces = set()
    for path in tracked_or_untracked_sources():
        relative = path.relative_to(ROOT)
        if path.name != "Cargo.lock" or "vendor" in relative.parts:
            continue
        parent = relative.parent.as_posix()
        workspaces.add("." if parent == "." else parent)
    return workspaces


def validate_cargo_workspaces(manifests: set[str]) -> list[str]:
    errors = []
    for relative in sorted(manifests):
        try:
            result = subprocess.run(
                [
                    "cargo",
                    "metadata",
                    "--no-deps",
                    "--format-version",
                    "1",
                    "--manifest-path",
                    relative,
                ],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
                timeout=60,
            )
            metadata = json.loads(result.stdout)
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
            detail = getattr(error, "stderr", None) or str(error)
            errors.append(f"Cargo workspace is invalid for {relative}: {str(detail).strip()}")
            continue
        workspace_manifest = (Path(metadata["workspace_root"]) / "Cargo.toml").resolve()
        package_manifests = {
            Path(package["manifest_path"]).resolve() for package in metadata.get("packages", [])
        }
        registered = (ROOT / relative).resolve()
        if registered != workspace_manifest and registered not in package_manifests:
            errors.append(f"Cargo metadata does not own registered manifest {relative}")
        if ROOT != workspace_manifest.parent and ROOT not in workspace_manifest.parents:
            errors.append(f"Cargo workspace for {relative} escapes the repository")
    return errors


def validate_cargo_check_delegations(
    registry: dict, suites: dict[str, dict], workspaces: set[str]
) -> list[str]:
    errors = []
    delegated = set()
    delegations = registry.get("cargo_check_delegations", [])
    if not isinstance(delegations, list):
        return ["cargo-check delegations must be an array of tables"]
    for index, delegation in enumerate(delegations):
        location = f"cargo-check delegation[{index}]"
        if not isinstance(delegation, dict):
            errors.append(f"{location} must be a table")
            continue
        workspace = delegation.get("workspace")
        suite = delegation.get("suite")
        reason = delegation.get("reason")
        if not isinstance(workspace, str) or workspace not in workspaces:
            errors.append(f"{location} names an unknown lockfile workspace")
        elif workspace in delegated:
            errors.append(f"duplicate cargo-check delegation for {workspace}")
        else:
            delegated.add(workspace)
        if not isinstance(suite, str) or suite not in suites:
            errors.append(f"{location} names an unknown validation suite")
        elif "pr" not in suites[suite].get("tiers", []):
            errors.append(f"{location} suite must run in the PR tier")
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f"{location} needs a non-empty reason")
    return errors


def validate_triage(path: Path | None = None) -> list[str]:
    errors: list[str] = []
    path = path or TRIAGE_PATH
    triage = load_toml(path)
    if triage.get("schema") != MUTATION_TRIAGE_SCHEMA:
        errors.append(f"mutation triage schema must be {MUTATION_TRIAGE_SCHEMA}")
    seen = set()
    today = dt.date.today()
    for index, accepted in enumerate(triage.get("accepted", [])):
        location = f"mutation triage accepted[{index}]"
        fingerprint = accepted.get("fingerprint")
        if not isinstance(fingerprint, str) or not re.fullmatch(r"[0-9a-f]{64}", fingerprint):
            errors.append(f"{location} needs a lowercase SHA-256 fingerprint")
        elif fingerprint in seen:
            errors.append(f"{location} duplicates fingerprint {fingerprint}")
        else:
            seen.add(fingerprint)
        for field in ("reason", "reviewer"):
            if not isinstance(accepted.get(field), str) or not accepted[field].strip():
                errors.append(f"{location} needs a non-empty {field}")
        try:
            expires = dt.date.fromisoformat(str(accepted.get("expires", "")))
            if expires < today:
                errors.append(f"{location} expired on {expires.isoformat()}")
        except ValueError:
            errors.append(f"{location} needs expires = YYYY-MM-DD")
    return errors


def validate_manifest(manifest: dict, check_tools: bool = False) -> list[str]:
    errors: list[str] = []
    for suite in manifest.get("suite", []):
        if "shards" not in suite:
            continue
        location = f"suite {suite.get('id')}"
        shards = suite["shards"]
        if isinstance(shards, bool) or not isinstance(shards, int) or not 2 <= shards <= 16:
            errors.append(f"{location} shards must be an integer from 2 through 16")
        if suite.get("domain") != "mutation":
            errors.append(f"{location} may shard only the mutation domain")
        command = suite.get("command")
        if isinstance(command, list) and any(
            part in {"--shard", "--sharding"} for part in command
        ):
            errors.append(f"{location} command must derive shard arguments from shards")
    try:
        named_toolchain(manifest, "nightly")
    except ValidationError as error:
        errors.append(str(error))
    try:
        suites = suite_map(manifest)
    except ValidationError as error:
        return [str(error)]

    registered_inputs: set[str] = set()
    for identifier, suite in suites.items():
        location = f"suite {identifier}"
        tiers = suite.get("tiers")
        if not isinstance(tiers, list) or not tiers or set(tiers) - VALID_TIERS:
            errors.append(f"{location} tiers must contain only {sorted(VALID_TIERS)}")
        if suite.get("platform") not in VALID_PLATFORMS:
            errors.append(f"{location} has invalid platform {suite.get('platform')!r}")
        if suite.get("toolchain") not in VALID_TOOLCHAINS:
            errors.append(f"{location} has invalid toolchain {suite.get('toolchain')!r}")
        command = suite.get("command")
        if not isinstance(command, list) or not command or not all(
            isinstance(part, str) and part for part in command
        ):
            errors.append(f"{location} needs a non-empty string command array")
        else:
            python_arguments = [
                (index, part)
                for index, part in enumerate(command)
                if PYTHON_ARGUMENT_PATTERN.search(part)
            ]
            for index, argument in python_arguments:
                if argument != RUNNER_PYTHON_ARGUMENT:
                    errors.append(f"{location} has unresolved Python argument {argument}")
                elif index != 0:
                    errors.append(
                        f"{location} may use {RUNNER_PYTHON_ARGUMENT} only as command element zero"
                    )
        timeout = suite.get("timeout_seconds")
        if not isinstance(timeout, int) or timeout <= 0:
            errors.append(f"{location} timeout_seconds must be positive")
        artifacts = suite.get("artifacts")
        if not isinstance(artifacts, str) or not artifacts.startswith("validation-artifacts/"):
            errors.append(f"{location} artifacts must live under validation-artifacts/")
        inputs = suite.get("inputs")
        if not isinstance(inputs, list) or not inputs:
            errors.append(f"{location} needs at least one input")
            continue
        for raw_path in inputs:
            if not isinstance(raw_path, str) or not raw_path:
                errors.append(f"{location} has an invalid input")
                continue
            registered_inputs.add(raw_path)
            if not (ROOT / raw_path).exists():
                errors.append(f"{location} input is missing: {raw_path}")
        working = suite.get("working_directory", ".")
        if not isinstance(working, str) or not (ROOT / working).is_dir():
            errors.append(f"{location} working directory is missing: {working}")

    expected_manifests = set(manifest.get("registry", {}).get("cargo_manifests", []))
    actual_manifests = source_cargo_manifests()
    if expected_manifests != actual_manifests:
        errors.append(
            "Cargo manifest registry drift: registry-only="
            f"{sorted(expected_manifests - actual_manifests)!r} source-only="
            f"{sorted(actual_manifests - expected_manifests)!r}"
        )
    else:
        errors.extend(validate_cargo_workspaces(expected_manifests))
    cargo_lock_workspaces = manifest.get("registry", {}).get("cargo_lock_workspaces", [])
    if not isinstance(cargo_lock_workspaces, list) or not cargo_lock_workspaces:
        errors.append("Cargo lockfile workspace registry must be a non-empty list")
    else:
        duplicates = sorted(
            path
            for path in set(cargo_lock_workspaces)
            if cargo_lock_workspaces.count(path) > 1
        )
        expected_locks = set(cargo_lock_workspaces)
        actual_locks = source_cargo_lock_workspaces()
        if duplicates:
            errors.append(f"duplicate Cargo lockfile workspaces: {duplicates!r}")
        if expected_locks != actual_locks:
            errors.append(
                "Cargo lockfile workspace registry drift: registry-only="
                f"{sorted(expected_locks - actual_locks)!r} source-only="
                f"{sorted(actual_locks - expected_locks)!r}"
            )
        errors.extend(
            validate_cargo_check_delegations(manifest["registry"], suites, expected_locks)
        )
    format_manifests = manifest.get("registry", {}).get("format_manifests", [])
    if not isinstance(format_manifests, list) or not format_manifests:
        errors.append("format manifest registry must be a non-empty list")
    else:
        duplicates = sorted(
            path for path in set(format_manifests) if format_manifests.count(path) > 1
        )
        unknown = sorted(set(format_manifests) - expected_manifests)
        if duplicates:
            errors.append(f"duplicate format manifests: {duplicates!r}")
        if unknown:
            errors.append(f"format manifests are not registered Cargo manifests: {unknown!r}")
        format_package_overrides = manifest.get("registry", {}).get(
            "format_package_overrides", {}
        )
        if not isinstance(format_package_overrides, dict):
            errors.append("format package overrides must be a table")
        else:
            unknown_overrides = sorted(
                set(format_package_overrides) - set(format_manifests)
            )
            if unknown_overrides:
                errors.append(
                    "format package overrides name unknown manifests: "
                    f"{unknown_overrides!r}"
                )
            for path, packages in format_package_overrides.items():
                if (
                    not isinstance(packages, list)
                    or not packages
                    or any(not isinstance(package, str) or not package for package in packages)
                    or len(set(packages)) != len(packages)
                ):
                    errors.append(
                        f"format package override for {path!r} must contain unique package names"
                    )

    try:
        discovered_kani = discover_kani_harnesses()
    except ValidationError as error:
        errors.append(str(error))
        discovered_kani = {}
    registered_kani = {entry.get("name"): entry.get("source") for entry in manifest.get("kani", [])}
    if registered_kani != discovered_kani:
        errors.append(
            "Kani registry drift: registry-only="
            f"{sorted(set(registered_kani) - set(discovered_kani))!r} source-only="
            f"{sorted(set(discovered_kani) - set(registered_kani))!r}"
        )

    fuzz_path = ROOT / "validation" / "fuzz" / "Cargo.toml"
    if fuzz_path.is_file():
        discovered_fuzz = discover_fuzz_targets(fuzz_path)
        registered_fuzz = {
            entry.get("name"): entry.get("path") for entry in manifest.get("fuzz_target", [])
        }
        if registered_fuzz != discovered_fuzz:
            errors.append(
                "fuzz registry drift: registry-only="
                f"{sorted(set(registered_fuzz) - set(discovered_fuzz))!r} source-only="
                f"{sorted(set(discovered_fuzz) - set(registered_fuzz))!r}"
            )

    exemptions = manifest.get("registry", {}).get("asset_exemptions", [])
    exempted = set()
    for exemption in exemptions:
        path = exemption.get("path")
        reason = exemption.get("reason")
        if not isinstance(path, str) or not path or not (ROOT / path).is_file():
            errors.append(f"invalid or missing asset exemption path: {path!r}")
        elif not isinstance(reason, str) or not reason.strip():
            errors.append(f"asset exemption {path} needs a reason")
        else:
            exempted.add(path)
    asset_inventory = validation_asset_inventory()
    orphaned = asset_inventory - registered_inputs - exempted
    stale_exemptions = exempted - asset_inventory
    if orphaned:
        errors.append(f"unregistered validation assets: {sorted(orphaned)!r}")
    if stale_exemptions:
        errors.append(f"stale validation asset exemptions: {sorted(stale_exemptions)!r}")

    errors.extend(validate_triage())
    requirements_versions: dict[str, set[str]] = {}
    for name, specification in manifest.get("interpreters", {}).items():
        version = specification.get("version")
        venv = specification.get("venv")
        requirements_path = specification.get("requirements")
        if not isinstance(version, str) or not version:
            errors.append(f"interpreter {name} needs an exact version")
            continue
        if not isinstance(venv, str) or "{version}" not in venv:
            errors.append(f"interpreter {name} venv must derive from {{version}}")
        if not isinstance(requirements_path, str) or not requirements_path:
            errors.append(f"interpreter {name} needs a requirements path")
            continue
        requirements_versions.setdefault(requirements_path, set()).add(version)
    for requirements_path, versions in sorted(requirements_versions.items()):
        if len(versions) != 1:
            errors.append(
                f"interpreters sharing {requirements_path} disagree on RNS versions: {sorted(versions)!r}"
            )
            continue
        expected_pin = f"rns=={next(iter(versions))}"
        requirements_file = ROOT / requirements_path
        lock_file = requirements_file.with_suffix(".lock")
        try:
            requirements = requirements_file.read_text().splitlines()
            lock = lock_file.read_text().splitlines()
        except OSError as error:
            errors.append(f"cannot read oracle reference pins: {error}")
            continue
        if requirements != lock:
            errors.append(
                f"{requirements_file.relative_to(ROOT)} must project {lock_file.relative_to(ROOT)}"
            )
        if expected_pin not in lock:
            errors.append(f"{lock_file.relative_to(ROOT)} must pin {expected_pin}")

    for suite in suites.values():
        uses_stock_peer = any(
            source.startswith("validation/interop/peers/rns_")
            and source.endswith(".py")
            for source in suite.get("inputs", [])
        )
        if uses_stock_peer and suite.get("interpreter") not in manifest.get("interpreters", {}):
            errors.append(f"{suite['id']} uses a stock RNS peer without a registered interpreter")

    required_commands = {
        sys.executable if suite["command"][0] == RUNNER_PYTHON_ARGUMENT else suite["command"][0]
        for suite in suites.values()
        if suite.get("command")
    }
    for command in sorted(required_commands):
        if "/" not in command and shutil.which(command) is None:
            errors.append(f"required command is unavailable: {command}")
    if check_tools:
        errors.extend(validate_tool_versions(manifest))
    return errors


def verification_report(manifest: dict, check_tools: bool) -> list[str]:
    suites = list(suite_map(manifest).values())
    tiers = {
        tier: sum(tier in suite["tiers"] for suite in suites) for tier in sorted(VALID_TIERS)
    }
    inputs = {
        path
        for suite in suites
        for path in suite.get("inputs", [])
        if isinstance(path, str)
    }
    registry = manifest["registry"]
    cargo_manifests = registry["cargo_manifests"]
    cargo_lock_workspaces = registry["cargo_lock_workspaces"]
    cargo_check_delegations = registry.get("cargo_check_delegations", [])
    format_manifests = registry["format_manifests"]
    kani = discover_kani_harnesses()
    fuzz = discover_fuzz_targets(ROOT / "validation" / "fuzz" / "Cargo.toml")
    assets = validation_asset_inventory()
    exemptions = registry.get("asset_exemptions", [])
    triage = load_toml(TRIAGE_PATH).get("accepted", [])
    commands = sorted({suite["command"][0] for suite in suites})
    interpreter_versions = sorted(
        {specification["version"] for specification in manifest.get("interpreters", {}).values()}
    )
    exemption_count = len(exemptions)
    exemption_phrase = (
        f"{exemption_count} documented exemption is"
        if exemption_count == 1
        else f"{exemption_count} documented exemptions are"
    )
    lines = [
        "[verify] Suite policy: "
        f"{len(suites)} total suites ({tiers['pr']} pull-request, {tiers['release']} release, "
        f"{tiers['scheduled']} scheduled); IDs, tiers, platforms, toolchains, commands, "
        "timeouts, and artifact paths are valid.",
        "[verify] Declared inputs: "
        f"{len(inputs)} unique files/directories exist; {len(commands)} required command "
        "entrypoints are available.",
        "[verify] Cargo ownership: "
        f"{len(cargo_manifests)} manifests are registered, valid, and repository-owned; "
        f"{len(cargo_lock_workspaces)} first-party lockfile workspaces are inventoried; "
        f"{len(cargo_lock_workspaces) - len(cargo_check_delegations)} enter the host compile sweep; "
        f"{len(cargo_check_delegations)} real-target workspaces are delegated; "
        f"{len(format_manifests)} unique workspace roots own formatting.",
        "[verify] Native discovery: "
        f"{len(kani)} Kani proofs and {len(fuzz)} fuzz targets exactly match their source owners.",
        "[verify] Asset ownership: "
        f"{len(assets)} oracle/interop/smoke assets are registered; {exemption_phrase} current; "
        "nothing is orphaned.",
        "[verify] External references: "
        f"stock RNS {', '.join(interpreter_versions)} is pinned for every registered interpreter.",
        "[verify] Mutation policy: "
        f"{len(triage)} accepted survivor entries; fingerprints, reasons, reviewers, and expiries "
        "are structurally current.",
    ]
    if check_tools:
        tools = manifest["tools"]
        lines.append(
            "[verify] Deep tools: installed versions match "
            f"cargo-fuzz {tools['cargo_fuzz']}, cargo-mutants {tools['cargo_mutants']}, "
            f"and Kani {tools['kani']}."
        )
    return lines


def validate_tool_versions(manifest: dict) -> list[str]:
    errors = []
    tools = manifest.get("tools", {})
    nightly = named_toolchain(manifest, "nightly")
    commands = {
        "cargo_fuzz": ["cargo", f"+{nightly}", "fuzz", "--version"],
        "cargo_mutants": ["cargo", "mutants", "--version"],
        "kani": ["cargo", "kani", "--version"],
    }
    for name, command in commands.items():
        expected = str(tools.get(name, ""))
        try:
            output = subprocess.run(
                command, cwd=ROOT, check=True, capture_output=True, text=True, timeout=30
            ).stdout
        except (OSError, subprocess.SubprocessError) as error:
            errors.append(f"cannot inspect {name}: {error}")
            continue
        if expected not in output:
            errors.append(f"{name} expected {expected}, got {output.strip()!r}")
    return errors


def selected_suites(
    manifest: dict,
    identifiers: list[str],
    domain: str | None,
    tier: str | None,
    platform: str | None = None,
) -> list[dict]:
    suites = suite_map(manifest)
    unknown = set(identifiers) - set(suites)
    if unknown:
        raise ValidationError(f"unknown suites: {sorted(unknown)!r}")
    selected = [suites[name] for name in identifiers] if identifiers else list(suites.values())
    if domain:
        selected = [suite for suite in selected if suite["domain"] == domain]
    if tier:
        selected = [suite for suite in selected if tier in suite["tiers"]]
    if platform == "current":
        host = native_platform()
        selected = [suite for suite in selected if suite["platform"] in {"any", host}]
    elif platform:
        selected = [suite for suite in selected if suite["platform"] == platform]
    return sorted(selected, key=lambda suite: suite["id"])


def ci_matrix(suites: list[dict]) -> dict:
    runners = {
        "any": "ubuntu-24.04",
        "linux": "ubuntu-24.04",
        "macos": "macos-14",
        "windows": "windows-2022",
        "android-device": ["self-hosted", "linux", "android", "prns-release"],
    }
    include = []
    for suite in suites:
        entry = dict(suite)
        entry["runner"] = runners[suite["platform"]]
        include.append(entry)
    return {"include": include}


def venv_python(venv: Path) -> Path:
    if os.name == "nt":
        return venv / "Scripts" / "python.exe"
    return venv / "bin" / "python"


def interpreter_venv(specification: dict) -> Path:
    return ROOT / specification["venv"].replace("{version}", specification["version"])


def resolve_interpreter(manifest: dict, name: str) -> str:
    specification = manifest.get("interpreters", {}).get(name)
    if not isinstance(specification, dict):
        raise ValidationError(f"unknown interpreter {name!r}")
    environment = specification["environment"]
    configured = os.environ.get(environment)
    candidate = Path(configured) if configured else venv_python(interpreter_venv(specification))
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise ValidationError(
            f"{environment} does not name an executable RNS interpreter; run "
            "`python3 validation/run.py prepare-oracles`"
        )
    version = subprocess.run(
        [str(candidate), "-c", "import RNS; print(RNS.__version__)"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.strip()
    if version != specification["version"]:
        raise ValidationError(
            f"{environment} uses RNS {version}, expected {specification['version']}; "
            "run `python3 validation/run.py prepare-oracles`"
        )
    return str(candidate)


def terminate(process: subprocess.Popen) -> tuple[bytes, bytes]:
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:  # pragma: no cover - exercised by the Windows release lane
            process.terminate()
    except ProcessLookupError:
        return process.communicate()
    try:
        return process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:  # pragma: no cover - exercised by the Windows release lane
                process.kill()
        except ProcessLookupError:
            pass
        return process.communicate()


def command_for(suite: dict, fuzz_seconds: int, artifact: Path) -> list[str]:
    artifact_root = artifact.parent.parent
    target = suite["id"].removeprefix("fuzz-")
    prefix = artifact_root / "fuzz" / target
    prefix.mkdir(parents=True, exist_ok=True)
    corpus = artifact_root / "fuzz-corpus" / target
    if "__FUZZ_CORPUS__" in suite["command"]:
        if corpus.exists():
            shutil.rmtree(corpus)
        corpus.mkdir(parents=True)
        seed_corpus = ROOT / "validation" / "fuzz" / "corpus" / target
        if seed_corpus.is_dir():
            shutil.copytree(seed_corpus, corpus, dirs_exist_ok=True)
    return [
        part.replace(RUNNER_PYTHON_ARGUMENT, sys.executable)
        .replace("__FUZZ_SECONDS__", str(fuzz_seconds))
        .replace("__FUZZ_ARTIFACT_PREFIX__", f"{prefix.as_posix()}/")
        .replace("__FUZZ_CORPUS__", corpus.as_posix())
        for part in suite["command"]
    ]


def command_version(command: list[str]) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return f"unavailable: {error}"
    return (result.stdout or result.stderr).strip().splitlines()[0]


def tool_versions(manifest: dict, suite: dict) -> dict[str, str]:
    versions = {
        "python": sys.version.splitlines()[0],
        "cargo": command_version(["cargo", "--version"]),
        "rustc": command_version(["rustc", "--version"]),
    }
    domain = suite["domain"]
    nightly = named_toolchain(manifest, "nightly")
    if domain == "fuzz":
        versions["cargo-fuzz"] = command_version(
            ["cargo", f"+{nightly}", "fuzz", "--version"]
        )
    elif domain == "kani":
        versions["kani"] = command_version(["cargo", "kani", "--version"])
    elif domain == "mutation":
        versions["cargo-mutants"] = command_version(["cargo", "mutants", "--version"])
    if suite.get("toolchain") == "nightly":
        versions["rustc-nightly"] = command_version(["rustc", f"+{nightly}", "--version"])
    if suite.get("toolchain") == "node" or suite.get("group") == "web":
        versions["node"] = command_version(["node", "--version"])
        versions["npm"] = command_version(["npm", "--version"])
    if suite.get("toolchain") == "dotnet":
        versions["dotnet"] = command_version(["dotnet", "--version"])
    return versions


def run_suite(manifest: dict, suite: dict, expected_sha: str | None, fuzz_seconds: int) -> bool:
    current_platform = native_platform()
    required_platform = suite["platform"]
    if required_platform == "android-device":
        if os.environ.get("PRNS_ANDROID_DEVICE") != "1":
            raise ValidationError(
                f"suite {suite['id']} requires PRNS_ANDROID_DEVICE=1 on a device-qualified runner"
            )
    elif required_platform not in {"any", current_platform}:
        raise ValidationError(
            f"suite {suite['id']} requires {required_platform}, current platform is {current_platform}"
        )
    commit = git_head()
    worktree_clean = tracked_worktree_is_clean()
    if expected_sha:
        validate_expected_sha(expected_sha)
        if commit != expected_sha:
            raise ValidationError(f"HEAD is {commit}, expected exact release SHA {expected_sha}")
        if not worktree_clean:
            raise ValidationError("exact-SHA evidence requires a clean tracked worktree")
    artifact_root = Path(os.environ.get("PRNS_VALIDATION_ARTIFACTS", ROOT / "validation-artifacts"))
    if not artifact_root.is_absolute():
        artifact_root = ROOT / artifact_root
    artifact = artifact_root / "results" / suite["id"]
    artifact.mkdir(parents=True, exist_ok=True)
    command = command_for(suite, fuzz_seconds, artifact)
    try:
        evidence_location = artifact.relative_to(ROOT).as_posix()
    except ValueError:
        evidence_location = artifact.as_posix()
    print(
        f"[run] {suite['id']}: domain={suite['domain']} platform={required_platform} "
        f"toolchain={suite['toolchain']} timeout={suite['timeout_seconds']}s"
    )
    print(f"[run] command: {shlex.join(command)}")
    print(f"[run] evidence: {evidence_location}")
    sys.stdout.flush()
    environment = os.environ.copy()
    environment["PRNS_VALIDATION_SUITE"] = suite["id"]
    if suite["domain"] == "mutation":
        mutation_output = artifact_root / "mutation" / suite["id"]
        mutation_output.parent.mkdir(parents=True, exist_ok=True)
        environment["PRNS_MUTANTS_OUTPUT_ROOT"] = str(mutation_output)
    resolved_interpreter = None
    if interpreter := suite.get("interpreter"):
        specification = manifest["interpreters"][interpreter]
        resolved_interpreter = resolve_interpreter(manifest, interpreter)
        environment[specification["environment"]] = resolved_interpreter
    versions = tool_versions(manifest, suite)
    if resolved_interpreter:
        versions["oracle-python"] = command_version([resolved_interpreter, "--version"])
        versions["stock-rns"] = f"RNS {specification['version']}"
    started_at = dt.datetime.now(dt.timezone.utc)
    started = time.monotonic()
    timed_out = False
    spawn_error = None
    exit_code = None
    stdout = b""
    stderr = b""
    try:
        process = subprocess.Popen(
            [shutil.which(command[0]) or command[0], *command[1:]],
            cwd=ROOT / suite.get("working_directory", "."),
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=suite["timeout_seconds"])
        except subprocess.TimeoutExpired:
            timed_out = True
            stdout, stderr = terminate(process)
        exit_code = process.returncode
    except OSError as error:
        spawn_error = str(error)
        stderr = spawn_error.encode()
    ending_worktree_clean = tracked_worktree_is_clean()
    finished_at = dt.datetime.now(dt.timezone.utc)
    duration_seconds = round(time.monotonic() - started, 3)
    if expected_sha and not ending_worktree_clean:
        stderr += b"\nvalidation failed: suite left tracked worktree changes\n"
    worktree_clean = worktree_clean and ending_worktree_clean
    passed = (
        exit_code == 0
        and not timed_out
        and spawn_error is None
        and (not expected_sha or worktree_clean)
    )
    (artifact / "stdout.log").write_bytes(stdout)
    (artifact / "stderr.log").write_bytes(stderr)
    result = {
        "schema": EVIDENCE_SCHEMA,
        "suite": suite["id"],
        "domain": suite["domain"],
        "commit": commit,
        "platform": current_platform,
        "required_platform": required_platform,
        "worktree_clean": worktree_clean,
        "command": command,
        "tool_versions": versions,
        "started_at": started_at.isoformat(),
        "finished_at": finished_at.isoformat(),
        "duration_seconds": duration_seconds,
        "status": "passed" if passed else "failed",
        "exit_code": exit_code,
        "timed_out": timed_out,
        "spawn_error": spawn_error,
    }
    if shard := suite.get("shard"):
        result["shard"] = shard
    (artifact / "result.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    sys.stdout.buffer.write(stdout)
    sys.stderr.buffer.write(stderr)
    print(f"VALIDATION_SUITE {suite['id']} {result['status']} {result['duration_seconds']}s")
    return passed


def normalized_mutant(mutant: dict) -> dict:
    name = str(mutant.get("name", ""))
    description = re.sub(r"^.*?:\d+:\d+:\s*", "", name)
    raw_function = mutant.get("function")
    if isinstance(raw_function, dict):
        function = {
            "name": raw_function.get("function_name"),
            "return_type": raw_function.get("return_type"),
        }
    else:
        function = raw_function
    return {
        "package": mutant.get("package"),
        "file": mutant.get("file"),
        "function": function,
        "genre": mutant.get("genre"),
        "replacement": mutant.get("replacement"),
        "description": description,
    }


def mutation_fingerprint(mutant: dict) -> str:
    encoded = json.dumps(normalized_mutant(mutant), sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def mutation_results_errors(payload: object, expected_version: str) -> list[str]:
    if not isinstance(payload, dict):
        return ["mutation outcomes must be a JSON object"]
    errors = []
    outcomes = payload.get("outcomes")
    if not isinstance(outcomes, list):
        return ["mutation outcomes must contain an outcomes array"]
    baselines = [
        outcome
        for outcome in outcomes
        if isinstance(outcome, dict) and outcome.get("scenario") == "Baseline"
    ]
    if len(baselines) != 1 or baselines[0].get("summary") != "Success":
        errors.append("mutation baseline did not complete successfully")
    mutants = [
        outcome
        for outcome in outcomes
        if isinstance(outcome, dict)
        and isinstance(outcome.get("scenario"), dict)
        and isinstance(outcome["scenario"].get("Mutant"), dict)
    ]
    total = payload.get("total_mutants")
    if isinstance(total, bool) or not isinstance(total, int) or total < 0:
        errors.append("mutation total_mutants must be a non-negative integer")
    elif len(mutants) != total:
        errors.append(f"mutation outcomes contain {len(mutants)} of {total} mutants")
    summaries = {
        "missed": "MissedMutant",
        "caught": "CaughtMutant",
        "timeout": "Timeout",
        "unviable": "Unviable",
    }
    recognized = set(summaries.values())
    unknown = sorted(
        {
            str(outcome.get("summary"))
            for outcome in mutants
            if outcome.get("summary") not in recognized
        }
    )
    if unknown:
        errors.append(f"mutation outcomes contain unknown summaries: {unknown!r}")
    for field, summary in summaries.items():
        reported = payload.get(field)
        actual = sum(outcome.get("summary") == summary for outcome in mutants)
        if isinstance(reported, bool) or not isinstance(reported, int) or reported != actual:
            errors.append(f"mutation {field} count is {reported!r}, expected {actual}")
    parsed_times = {}
    for field in ("start_time", "end_time"):
        value = payload.get(field)
        if not isinstance(value, str) or not value:
            errors.append(
                "mutation run is incomplete"
                if field == "end_time"
                else "mutation start_time is missing"
            )
            continue
        try:
            matched = MUTATION_TIME_PATTERN.fullmatch(value)
            if matched is None:
                raise ValueError
            fraction = matched.group("fraction")
            normalized = matched.group("prefix")
            if fraction:
                normalized += f".{fraction[:6].ljust(6, '0')}"
            zone = matched.group("zone")
            normalized += "+00:00" if zone == "Z" else zone
            parsed = dt.datetime.fromisoformat(normalized)
            parsed_times[field] = parsed
        except ValueError:
            errors.append(f"mutation {field} is not timezone-aware ISO-8601")
    if set(parsed_times) == {"start_time", "end_time"} and (
        parsed_times["end_time"] < parsed_times["start_time"]
    ):
        errors.append("mutation end_time precedes start_time")
    success = payload.get("success")
    if isinstance(success, bool) or not isinstance(success, int) or success < 0:
        errors.append("mutation success count must be a non-negative integer")
    if payload.get("cargo_mutants_version") != expected_version:
        errors.append(
            "mutation cargo-mutants version is "
            f"{payload.get('cargo_mutants_version')!r}, expected {expected_version!r}"
        )
    return errors


def unresolved_mutants_from(payload: dict) -> dict[str, dict]:
    unresolved = {}
    for outcome in payload.get("outcomes", []):
        if not isinstance(outcome, dict):
            continue
        if outcome.get("summary") not in {"MissedMutant", "Timeout"}:
            continue
        scenario = outcome.get("scenario", {})
        mutant = scenario.get("Mutant") if isinstance(scenario, dict) else None
        if isinstance(mutant, dict):
            unresolved[mutation_fingerprint(mutant)] = mutant
    return unresolved


def unresolved_mutants(results: Path) -> dict[str, dict]:
    return unresolved_mutants_from(json.loads(results.read_text(encoding="utf-8")))


def check_mutation_triage(results: Path, expected_version: str) -> list[str]:
    triage_errors = validate_triage()
    payload = json.loads(results.read_text(encoding="utf-8"))
    errors = [*triage_errors, *mutation_results_errors(payload, expected_version)]
    if triage_errors:
        return errors
    triage = load_toml(TRIAGE_PATH)
    accepted = {entry["fingerprint"]: entry for entry in triage.get("accepted", [])}
    unresolved = unresolved_mutants_from(payload)
    for fingerprint, mutant in sorted(unresolved.items()):
        if fingerprint not in accepted:
            errors.append(f"untriaged mutant {fingerprint}: {mutant.get('name')}")
    stale = set(accepted) - set(unresolved)
    if stale:
        errors.append(f"stale mutation triage entries: {sorted(stale)!r}")
    return errors


def merged_mutation_results(
    payloads: list[dict], shards: list[dict], expected_version: str
) -> dict:
    baselines = [
        outcome
        for payload in payloads
        for outcome in payload["outcomes"]
        if outcome.get("scenario") == "Baseline"
    ]
    mutants = [
        outcome
        for payload in payloads
        for outcome in payload["outcomes"]
        if isinstance(outcome.get("scenario"), dict)
        and isinstance(outcome["scenario"].get("Mutant"), dict)
    ]
    summaries = {
        "missed": "MissedMutant",
        "caught": "CaughtMutant",
        "timeout": "Timeout",
        "unviable": "Unviable",
    }
    return {
        "schema": EVIDENCE_SCHEMA,
        "shards": shards,
        "outcomes": [baselines[0], *mutants],
        "total_mutants": len(mutants),
        **{
            field: sum(outcome.get("summary") == summary for outcome in mutants)
            for field, summary in summaries.items()
        },
        "success": sum(int(payload.get("success", 0)) for payload in payloads),
        "start_time": min(payload["start_time"] for payload in payloads),
        "end_time": max(payload["end_time"] for payload in payloads),
        "cargo_mutants_version": expected_version,
    }


def cleanup_paths(manifest: dict) -> list[Path]:
    candidates = {
        ROOT / Path(cargo_manifest).parent / "target"
        for cargo_manifest in manifest.get("registry", {}).get("cargo_manifests", [])
    }
    candidates.update(
        ROOT / path
        for path in (
            "validation/fuzz/artifacts",
            "validation/fuzz/coverage",
            "validation/.venv",
            "validation-artifacts",
            "mutants.out",
            "mutants.out.old",
            "docs/website/node_modules",
            "docs/website/dist",
            "docs/website/pkg",
            "prns-wasm/node_modules",
            "prns-wasm/smoke/dist",
            "prns-wasm/smoke/pkg",
            "personal-hopspot/mobile/android/.gradle",
            "personal-hopspot/mobile/android/.kotlin",
            "personal-hopspot/mobile/android/build",
            "personal-hopspot/mobile/android/app/build",
            "personal-hopspot/mobile/android/app/src/main/jniLibs",
            "benchmarks/reference/.venv",
            ".venv-rns-1.4.0",
        )
    )
    for name in ("__pycache__", ".pytest_cache"):
        candidates.update(
            path
            for path in ROOT.rglob(name)
            if path.is_dir()
            and ".upstream" not in path.relative_to(ROOT).parts
            and not any(part.startswith(".venv") for part in path.relative_to(ROOT).parts)
        )
    safe = []
    for path in candidates:
        resolved = path.resolve()
        if ROOT not in resolved.parents or resolved == ROOT:
            raise ValidationError(f"refusing unsafe cleanup path {path}")
        if path.exists() or path.is_symlink():
            safe.append(path)
    ordered = sorted(safe, key=lambda path: (len(path.parts), path.as_posix()))
    collapsed: list[Path] = []
    for path in ordered:
        if not any(parent == path or parent in path.parents for parent in collapsed):
            collapsed.append(path)
    return sorted(collapsed)


def cleanup(manifest: dict, apply: bool) -> None:
    paths = cleanup_paths(manifest)
    if not paths:
        print("VALIDATION_CLEANUP_EMPTY")
        return
    mode = "apply" if apply else "dry-run"
    print(
        f"[cleanup] mode={mode}; {len(paths)} generated output roots selected; "
        "source corpora, credentials, editor settings, and runtime identity/state are protected."
    )
    for path in paths:
        relative = path.relative_to(ROOT)
        if apply:
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
            print(f"VALIDATION_CLEANUP_REMOVED {relative}")
        else:
            print(f"VALIDATION_CLEANUP_WOULD_REMOVE {relative}")
    if not apply:
        print("Re-run with `python3 validation/run.py cleanup --apply` to remove these outputs.")


def evidence_errors(result: object) -> list[str]:
    if not isinstance(result, dict):
        return ["evidence must be a JSON object"]
    required = {
        "schema",
        "suite",
        "domain",
        "commit",
        "platform",
        "required_platform",
        "worktree_clean",
        "command",
        "tool_versions",
        "started_at",
        "finished_at",
        "duration_seconds",
        "status",
        "exit_code",
        "timed_out",
        "spawn_error",
    }
    errors = []
    missing = sorted(required - set(result))
    unexpected = sorted(set(result) - required - {"shard"})
    if missing:
        errors.append(f"missing fields: {missing!r}")
    if unexpected:
        errors.append(f"unexpected fields: {unexpected!r}")
    if result.get("schema") != EVIDENCE_SCHEMA:
        errors.append(f"incompatible evidence schema {result.get('schema')!r}")
    for field in ("suite", "domain", "platform", "required_platform"):
        if not isinstance(result.get(field), str) or not result[field]:
            errors.append(f"{field} must be a non-empty string")
    if not isinstance(result.get("commit"), str) or not re.fullmatch(
        r"[0-9a-f]{40}", result["commit"]
    ):
        errors.append("commit must be a lowercase full 40-character SHA")
    if not isinstance(result.get("worktree_clean"), bool):
        errors.append("worktree_clean must be a boolean")
    command = result.get("command")
    if not isinstance(command, list) or not command or not all(
        isinstance(part, str) and part for part in command
    ):
        errors.append("command must be a non-empty string array")
    versions = result.get("tool_versions")
    if not isinstance(versions, dict) or not versions:
        errors.append("tool_versions must be a non-empty object")
    elif any(
        not isinstance(name, str)
        or not name
        or not isinstance(version, str)
        or not version
        or version.startswith("unavailable:")
        for name, version in versions.items()
    ):
        errors.append("tool_versions contains an invalid or unavailable tool")
    parsed_times = {}
    for field in ("started_at", "finished_at"):
        try:
            parsed = dt.datetime.fromisoformat(result.get(field, ""))
            if parsed.tzinfo is None:
                raise ValueError
            parsed_times[field] = parsed
        except (TypeError, ValueError):
            errors.append(f"{field} must be a timezone-aware ISO-8601 timestamp")
    if set(parsed_times) == {"started_at", "finished_at"} and (
        parsed_times["finished_at"] < parsed_times["started_at"]
    ):
        errors.append("finished_at precedes started_at")
    duration = result.get("duration_seconds")
    if isinstance(duration, bool) or not isinstance(duration, (int, float)) or duration < 0:
        errors.append("duration_seconds must be a non-negative number")
    if result.get("status") not in {"passed", "failed"}:
        errors.append("status must be passed or failed")
    exit_code = result.get("exit_code")
    if isinstance(exit_code, bool) or not (exit_code is None or isinstance(exit_code, int)):
        errors.append("exit_code must be an integer or null")
    if not isinstance(result.get("timed_out"), bool):
        errors.append("timed_out must be a boolean")
    if result.get("spawn_error") is not None and not isinstance(result.get("spawn_error"), str):
        errors.append("spawn_error must be a string or null")
    if "shard" in result:
        shard = result["shard"]
        if (
            not isinstance(shard, dict)
            or set(shard) != {"suite", "index", "total"}
            or not isinstance(shard.get("suite"), str)
            or not shard.get("suite")
            or isinstance(shard.get("index"), bool)
            or not isinstance(shard.get("index"), int)
            or isinstance(shard.get("total"), bool)
            or not isinstance(shard.get("total"), int)
            or not 0 <= shard.get("index", -1) < shard.get("total", 0)
        ):
            errors.append("shard must identify one valid suite index and total")
    if result.get("status") == "passed" and (
        exit_code != 0 or result.get("timed_out") is not False or result.get("spawn_error") is not None
    ):
        errors.append("passed evidence has a non-success process state")
    return errors


def aggregate_mutation_results(
    manifest: dict,
    artifact_root: Path,
    suites: list[dict],
    results: dict[str, dict],
) -> tuple[Path | None, list[str]]:
    if not suites:
        return None, []
    errors = []
    expected_version = manifest["tools"]["cargo_mutants"]
    expected_identities = {
        (
            suite["shard"]["suite"],
            suite["shard"]["index"],
            suite["shard"]["total"],
        )
        for suite in suites
    }
    observed_identities = set()
    payloads = []
    shard_records = []
    for suite in suites:
        identifier = suite["id"]
        result = results.get(identifier)
        if result is None:
            continue
        shard = result.get("shard")
        if (
            isinstance(shard, dict)
            and isinstance(shard.get("suite"), str)
            and isinstance(shard.get("index"), int)
            and not isinstance(shard.get("index"), bool)
            and isinstance(shard.get("total"), int)
            and not isinstance(shard.get("total"), bool)
        ):
            identity = (shard.get("suite"), shard.get("index"), shard.get("total"))
            if identity in observed_identities:
                errors.append(f"duplicate mutation shard identity {identity!r}")
            observed_identities.add(identity)
        if shard != suite["shard"]:
            errors.append(
                f"{identifier} mutation shard identity is {shard!r}, "
                f"expected {suite['shard']!r}"
            )
        if result.get("command") != suite["command"]:
            errors.append(
                f"{identifier} mutation shard command is {result.get('command')!r}, "
                f"expected {suite['command']!r}"
            )
        path = (
            artifact_root
            / "mutation"
            / identifier
            / "mutants.out"
            / "outcomes.json"
        )
        if not path.is_file():
            errors.append(f"missing mutation outcomes for {identifier}")
            continue
        payload = json.loads(path.read_text(encoding="utf-8"))
        errors.extend(
            f"{identifier}: {error}"
            for error in mutation_results_errors(payload, expected_version)
        )
        payloads.append(payload)
        shard_records.append({"validation_suite": identifier, **suite["shard"]})
    missing_identities = expected_identities - observed_identities
    unexpected_identities = observed_identities - expected_identities
    if missing_identities:
        errors.append(
            f"missing mutation shard identities: {sorted(missing_identities, key=repr)!r}"
        )
    if unexpected_identities:
        errors.append(
            "unexpected mutation shard identities: "
            f"{sorted(unexpected_identities, key=repr)!r}"
        )
    if errors or len(payloads) != len(suites):
        return None, errors
    union = merged_mutation_results(payloads, shard_records, expected_version)
    output = artifact_root / "mutation" / "union" / "outcomes.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(union, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    errors.extend(check_mutation_triage(output, expected_version))
    return output, errors


def aggregate(manifest: dict, expected_sha: str, tier: str, domain: str | None) -> Path:
    validate_expected_sha(expected_sha)
    registry_errors = validate_manifest(manifest)
    if registry_errors:
        raise ValidationError("\n".join(registry_errors))
    if git_head() != expected_sha:
        raise ValidationError(f"aggregate checkout is not exact commit {expected_sha}")
    if not tracked_worktree_is_clean():
        raise ValidationError("aggregate requires a clean tracked worktree")
    artifact_root = Path(os.environ.get("PRNS_VALIDATION_ARTIFACTS", ROOT / "validation-artifacts"))
    if not artifact_root.is_absolute():
        artifact_root = ROOT / artifact_root
    required = selected_suites(manifest, [], domain, tier)
    scope = f"domain={domain}" if domain else "all registered domains"
    print(
        f"[aggregate] Requiring {len(required)} {tier}-tier suites from {scope} "
        f"at exact commit {expected_sha}."
    )
    results = {}
    errors = []
    for suite in required:
        path = artifact_root / "results" / suite["id"] / "result.json"
        if not path.is_file():
            errors.append(f"missing result for {suite['id']}")
            continue
        result = json.loads(path.read_text(encoding="utf-8"))
        results[suite["id"]] = result
        errors.extend(f"{suite['id']}: {error}" for error in evidence_errors(result))
        if result.get("suite") != suite["id"] or result.get("domain") != suite["domain"]:
            errors.append(f"{suite['id']} evidence identity does not match the registry")
        if result.get("status") != "passed":
            errors.append(f"{suite['id']} did not pass")
        if result.get("commit") != expected_sha:
            errors.append(f"{suite['id']} is bound to {result.get('commit')}, expected {expected_sha}")
        if result.get("worktree_clean") is not True:
            errors.append(f"{suite['id']} was not executed from a clean tracked worktree")
        if not isinstance(result.get("tool_versions"), dict) or not result["tool_versions"]:
            errors.append(f"{suite['id']} is missing tool-version evidence")
    mutation_union, mutation_errors = aggregate_mutation_results(
        manifest,
        artifact_root,
        [suite for suite in required if suite["domain"] == "mutation"],
        results,
    )
    errors.extend(mutation_errors)
    if errors:
        raise ValidationError("\n".join(errors))
    output = artifact_root / "release-manifest.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(
            {
                "schema": EVIDENCE_SCHEMA,
                "commit": expected_sha,
                "tier": tier,
                "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                "results": results,
                **(
                    {"mutation_union": mutation_union.relative_to(artifact_root).as_posix()}
                    if mutation_union
                    else {}
                ),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return output


def prepare_oracles(manifest: dict) -> None:
    uv = shutil.which("uv")
    if uv is None:
        raise ValidationError("uv is required to prepare pinned oracle environments")
    for name, specification in manifest.get("interpreters", {}).items():
        venv = interpreter_venv(specification)
        print(
            f"[oracles] {name}: creating {venv.relative_to(ROOT)} with stock RNS "
            f"{specification['version']} from {specification['requirements']}."
        )
        subprocess.run([uv, "venv", "--clear", str(venv)], cwd=ROOT, check=True)
        subprocess.run(
            [
                uv,
                "pip",
                "install",
                "--python",
                str(venv_python(venv)),
                "-r",
                str(ROOT / specification["requirements"]),
            ],
            cwd=ROOT,
            check=True,
        )
        print(f"ORACLE_INTERPRETER_READY {name} {venv}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    verify = subcommands.add_parser("verify")
    verify.add_argument("--check-tools", action="store_true")
    list_command = subcommands.add_parser("list")
    list_command.add_argument("--domain")
    list_command.add_argument("--tier", choices=sorted(VALID_TIERS))
    list_command.add_argument("--platform", choices=["current", *sorted(VALID_PLATFORMS)])
    matrix = subcommands.add_parser("matrix")
    matrix.add_argument("--domain")
    matrix.add_argument("--tier", choices=sorted(VALID_TIERS))
    matrix.add_argument("--platform", choices=["current", *sorted(VALID_PLATFORMS)])
    run = subcommands.add_parser("run")
    run.add_argument("--suite", action="append", default=[])
    run.add_argument("--domain")
    run.add_argument("--tier", choices=sorted(VALID_TIERS))
    run.add_argument("--platform", choices=["current", *sorted(VALID_PLATFORMS)])
    run.add_argument("--expected-sha")
    run.add_argument("--fuzz-seconds", type=int, default=int(os.environ.get("PRNS_FUZZ_SECONDS", "30")))
    toolchain = subcommands.add_parser("toolchain")
    toolchain.add_argument("name", choices=["nightly"])
    subcommands.add_parser("prepare-oracles")
    mutation = subcommands.add_parser("mutation-check")
    mutation.add_argument("--results", type=Path, required=True)
    mutation_shard = subcommands.add_parser("mutation-shard-check")
    mutation_shard.add_argument("--results", type=Path, required=True)
    aggregate_command = subcommands.add_parser("aggregate")
    aggregate_command.add_argument("--expected-sha", required=True)
    aggregate_command.add_argument("--tier", choices=sorted(VALID_TIERS), default="release")
    aggregate_command.add_argument("--domain")
    cleanup_command = subcommands.add_parser("cleanup")
    cleanup_command.add_argument("--apply", action="store_true")
    return parser


def main() -> int:
    arguments = build_parser().parse_args()
    try:
        manifest = load_manifest()
        if arguments.command == "verify":
            errors = validate_manifest(manifest, check_tools=arguments.check_tools)
            if errors:
                raise ValidationError("\n".join(errors))
            for line in verification_report(manifest, arguments.check_tools):
                print(line)
            print("VALIDATION_REGISTRY_OK")
        elif arguments.command in {"list", "matrix"}:
            suites = selected_suites(
                manifest, [], arguments.domain, arguments.tier, arguments.platform
            )
            if arguments.command == "matrix":
                runners = set()
                for entry in ci_matrix(suites)["include"]:
                    runner = entry["runner"]
                    runners.add(" + ".join(runner) if isinstance(runner, list) else runner)
                print(
                    f"[matrix] {len(suites)} suites selected; "
                    f"runners={', '.join(sorted(runners))}; "
                    "stdout remains CI-ready JSON.",
                    file=sys.stderr,
                )
                print(json.dumps(ci_matrix(suites), sort_keys=True))
            else:
                filters = []
                if arguments.domain:
                    filters.append(f"domain={arguments.domain}")
                if arguments.tier:
                    filters.append(f"tier={arguments.tier}")
                if arguments.platform:
                    filters.append(f"platform={arguments.platform}")
                print(
                    f"[list] {len(suites)} suites selected"
                    + (f" ({', '.join(filters)})" if filters else "")
                    + "; columns include the exact registered command.",
                    file=sys.stderr,
                )
                print("id\tdomain\ttiers\tplatform\ttimeout_seconds\tcommand")
                for suite in suites:
                    print(
                        "\t".join(
                            [
                                suite["id"],
                                suite["domain"],
                                ",".join(suite["tiers"]),
                                suite["platform"],
                                str(suite["timeout_seconds"]),
                                " ".join(suite["command"]),
                            ]
                        )
                    )
        elif arguments.command == "run":
            errors = validate_manifest(manifest)
            if errors:
                raise ValidationError("\n".join(errors))
            suites = selected_suites(
                manifest,
                arguments.suite,
                arguments.domain,
                arguments.tier,
                arguments.platform,
            )
            if not suites:
                raise ValidationError("suite selection is empty")
            custody = f"exact SHA {arguments.expected_sha}" if arguments.expected_sha else "development"
            suite_label = "suite" if len(suites) == 1 else "suites"
            print(
                f"[run] Plan: {len(suites)} {suite_label}, custody={custody}; all selected suites "
                "will be attempted even if one fails."
            )
            results = [
                run_suite(manifest, suite, arguments.expected_sha, arguments.fuzz_seconds)
                for suite in suites
            ]
            passed = all(results)
            return 0 if passed else 1
        elif arguments.command == "toolchain":
            print(named_toolchain(manifest, arguments.name))
        elif arguments.command == "prepare-oracles":
            prepare_oracles(manifest)
        elif arguments.command == "mutation-shard-check":
            payload = json.loads(arguments.results.read_text(encoding="utf-8"))
            errors = mutation_results_errors(payload, manifest["tools"]["cargo_mutants"])
            if errors:
                raise ValidationError("\n".join(errors))
            print(
                f"MUTATION_SHARD_OK mutants={payload['total_mutants']} "
                f"missed={payload['missed']} timeout={payload['timeout']}"
            )
        elif arguments.command == "mutation-check":
            errors = check_mutation_triage(
                arguments.results, manifest["tools"]["cargo_mutants"]
            )
            if errors:
                raise ValidationError("\n".join(errors))
            unresolved = unresolved_mutants(arguments.results)
            accepted = load_toml(TRIAGE_PATH).get("accepted", [])
            print(
                f"MUTATION_TRIAGE_OK unresolved={len(unresolved)} accepted={len(accepted)}; "
                "the sets match exactly and every acceptance is reviewed and unexpired."
            )
        elif arguments.command == "aggregate":
            output = aggregate(manifest, arguments.expected_sha, arguments.tier, arguments.domain)
            suites = selected_suites(manifest, [], arguments.domain, arguments.tier)
            print(
                f"VALIDATION_RELEASE_READY suites={len(suites)} commit={arguments.expected_sha}"
            )
            print(f"VALIDATION_RELEASE_MANIFEST {output}")
        elif arguments.command == "cleanup":
            cleanup(manifest, arguments.apply)
    except (ValidationError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
