#!/usr/bin/env python3
"""Compile-check every host-compatible first-party Cargo workspace."""

from __future__ import annotations

import argparse
import os
import platform
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - repository Python includes tomllib
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        print(
            "CARGO_CHECK_ERROR: Python 3.11+ (or the tomli package) is required",
            file=sys.stderr,
        )
        raise SystemExit(1)


ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = ROOT / "validation" / "manifest.toml"


class CargoCheckConfigurationError(RuntimeError):
    pass


@dataclass(frozen=True)
class CargoCheckDelegation:
    workspace: str
    suite: str
    reason: str


@dataclass(frozen=True)
class CargoCheckPlan:
    workspaces: tuple[str, ...]
    delegations: tuple[CargoCheckDelegation, ...]


@dataclass(frozen=True)
class CargoCheckResult:
    workspace: str
    command: tuple[str, ...]
    returncode: int
    duration_seconds: float
    stdout: str
    stderr: str


def host_platform() -> str:
    system = platform.system().lower()
    return {"darwin": "macos"}.get(system, system)


def positive_integer(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def default_parallelism() -> tuple[int, int]:
    logical_cpus = os.cpu_count() or 2
    workspace_jobs = max(1, min(8, logical_cpus // 2))
    cargo_jobs = max(1, logical_cpus // workspace_jobs)
    return workspace_jobs, cargo_jobs


def load_plan(root: Path = ROOT, manifest_path: Path = MANIFEST_PATH) -> CargoCheckPlan:
    try:
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise CargoCheckConfigurationError(f"cannot load {manifest_path}: {error}") from error
    if manifest.get("schema") != 1:
        raise CargoCheckConfigurationError("validation manifest schema must be 1")

    registry = manifest.get("registry", {})
    raw_workspaces = registry.get("cargo_lock_workspaces")
    if not isinstance(raw_workspaces, list) or not raw_workspaces:
        raise CargoCheckConfigurationError("cargo_lock_workspaces must be a non-empty list")
    if any(
        not isinstance(workspace, str) or not workspace for workspace in raw_workspaces
    ):
        raise CargoCheckConfigurationError("cargo_lock_workspaces must contain non-empty paths")
    if len(set(raw_workspaces)) != len(raw_workspaces):
        raise CargoCheckConfigurationError("cargo_lock_workspaces contains duplicates")

    workspace_set = set(raw_workspaces)
    for workspace in raw_workspaces:
        relative = Path(workspace)
        if relative.is_absolute() or ".." in relative.parts:
            raise CargoCheckConfigurationError(f"workspace escapes the repository: {workspace}")
        workspace_root = root / relative
        if not (workspace_root / "Cargo.toml").is_file():
            raise CargoCheckConfigurationError(f"workspace has no Cargo.toml: {workspace}")
        if not (workspace_root / "Cargo.lock").is_file():
            raise CargoCheckConfigurationError(f"workspace has no Cargo.lock: {workspace}")

    suites = {
        suite.get("id"): suite
        for suite in manifest.get("suite", [])
        if isinstance(suite, dict) and isinstance(suite.get("id"), str)
    }
    delegations = []
    delegated_workspaces = set()
    for index, raw_delegation in enumerate(registry.get("cargo_check_delegations", [])):
        location = f"cargo_check_delegations[{index}]"
        if not isinstance(raw_delegation, dict):
            raise CargoCheckConfigurationError(f"{location} must be a table")
        workspace = raw_delegation.get("workspace")
        suite = raw_delegation.get("suite")
        reason = raw_delegation.get("reason")
        if not isinstance(workspace, str) or workspace not in workspace_set:
            raise CargoCheckConfigurationError(f"{location} names an unknown workspace")
        if workspace in delegated_workspaces:
            raise CargoCheckConfigurationError(f"duplicate cargo-check delegation: {workspace}")
        if not isinstance(suite, str) or suite not in suites:
            raise CargoCheckConfigurationError(f"{location} names an unknown suite")
        if "pr" not in suites[suite].get("tiers", []):
            raise CargoCheckConfigurationError(f"{location} suite must run in the PR tier")
        if not isinstance(reason, str) or not reason.strip():
            raise CargoCheckConfigurationError(f"{location} needs a reason")
        delegated_workspaces.add(workspace)
        delegations.append(CargoCheckDelegation(workspace, suite, reason))

    checked = tuple(
        workspace for workspace in raw_workspaces if workspace not in delegated_workspaces
    )
    return CargoCheckPlan(checked, tuple(delegations))


def run_workspace(
    root: Path,
    workspace: str,
    cargo_jobs: int,
    target_directory: Path,
) -> CargoCheckResult:
    command = (
        "cargo",
        "check",
        "--workspace",
        "--all-targets",
        "--locked",
        "--jobs",
        str(cargo_jobs),
    )
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_directory)
    environment.setdefault("CARGO_TERM_COLOR", "never")
    started = time.monotonic()
    try:
        result = subprocess.run(
            command,
            cwd=root / workspace,
            env=environment,
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
        )
        returncode = result.returncode
        stdout = result.stdout
        stderr = result.stderr
    except OSError as error:
        returncode = 127
        stdout = ""
        stderr = str(error)
    return CargoCheckResult(
        workspace,
        command,
        returncode,
        time.monotonic() - started,
        stdout,
        stderr,
    )


def execute_plan(
    plan: CargoCheckPlan,
    root: Path,
    workspace_jobs: int,
    cargo_jobs: int,
) -> int:
    current_platform = host_platform()
    target_directory = root / "target" / "repo-cargo-check" / current_platform
    print(
        f"[cargo-check] platform={current_platform}; workspaces={len(plan.workspaces)}; "
        f"parallel-workspaces={workspace_jobs}; cargo-jobs={cargo_jobs}"
    )
    for delegation in plan.delegations:
        print(
            f"[cargo-check] delegated {delegation.workspace} -> {delegation.suite}: "
            f"{delegation.reason}"
        )
    sys.stdout.flush()

    failures = []
    with ThreadPoolExecutor(max_workers=workspace_jobs) as executor:
        futures = {
            executor.submit(
                run_workspace,
                root,
                workspace,
                cargo_jobs,
                target_directory,
            ): workspace
            for workspace in plan.workspaces
        }
        for future in as_completed(futures):
            result = future.result()
            status = "PASS" if result.returncode == 0 else "FAIL"
            print(
                f"[cargo-check] {status} {result.workspace} "
                f"({result.duration_seconds:.1f}s)"
            )
            sys.stdout.flush()
            if result.returncode != 0:
                failures.append(result)

    for failure in sorted(failures, key=lambda result: result.workspace):
        print(f"\n[cargo-check] failure: {failure.workspace}", file=sys.stderr)
        print(f"[cargo-check] command: {' '.join(failure.command)}", file=sys.stderr)
        if failure.stdout:
            print(
                failure.stdout,
                file=sys.stderr,
                end="" if failure.stdout.endswith("\n") else "\n",
            )
        if failure.stderr:
            print(
                failure.stderr,
                file=sys.stderr,
                end="" if failure.stderr.endswith("\n") else "\n",
            )

    if failures:
        failed = ", ".join(sorted(failure.workspace for failure in failures))
        print(f"CARGO_CHECK_FAILED: {failed}", file=sys.stderr)
        return 1
    print(
        f"CARGO_CHECK_OK: {len(plan.workspaces)} host workspaces passed; "
        f"{len(plan.delegations)} real-target workspaces delegated."
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    default_workspace_jobs, default_cargo_jobs = default_parallelism()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--jobs",
        type=positive_integer,
        default=default_workspace_jobs,
        help="maximum Cargo workspaces checked concurrently",
    )
    parser.add_argument(
        "--cargo-jobs",
        type=positive_integer,
        default=default_cargo_jobs,
        help="parallel compiler jobs allowed inside each Cargo process",
    )
    arguments = parser.parse_args(argv)
    try:
        plan = load_plan()
    except CargoCheckConfigurationError as error:
        print(f"CARGO_CHECK_ERROR: {error}", file=sys.stderr)
        return 1
    return execute_plan(plan, ROOT, arguments.jobs, arguments.cargo_jobs)


if __name__ == "__main__":
    raise SystemExit(main())
