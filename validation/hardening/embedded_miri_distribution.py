"""Execute registered Miri shards and compose only complete, matching evidence."""
from __future__ import annotations

import json
import os
import platform
import subprocess
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict
from pathlib import Path

from validation import run as control
from validation.hardening import embedded_miri as miri
from validation.hardening.embedded_failure import ProofExecutionError
from validation.hardening.embedded_miri_shards import (
    Shard, ShardError, collect_model, write_fragment,
)


BUILD_FLAGS = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET")
SHARD_WORKERS_PER_MODEL = 2
MAX_LOCAL_SHARDS = 2


def local_parallelism() -> int:
    models = len(miri.mode_configuration(miri.Mode.FULL).borrow_models)
    processors_per_shard = models * SHARD_WORKERS_PER_MODEL
    return max(1, min(MAX_LOCAL_SHARDS, (os.cpu_count() or 1) // processors_per_shard))


def clean_commit(expected: str | None = None) -> str:
    commit = control.git_head()
    if expected is not None and commit != expected:
        raise ShardError(f"Miri checkout is {commit}, expected {expected}")
    if not control.tracked_worktree_is_clean():
        raise ShardError("distributed Miri evidence requires a clean tracked worktree")
    return commit


def constant_context(scenario: miri.Scenario, model: miri.BorrowModel, commit: str) -> dict:
    return {
        "commit": commit,
        "component": scenario.component,
        "scenario": scenario.identifier,
        "model": model.value,
        "miri_flags": list(miri.miri_flags(model)),
        "manifest": miri.relative(scenario.manifest),
        "features": list(scenario.features),
        "filters": list(scenario.full_filters),
        "sharding": control.ROUND_ROBIN_SHARDING,
    }


def run_shard(shard: Shard) -> None:
    commit = clean_commit()
    specifications = [
        suite for suite in control.load_manifest()["suite"]
        if suite["id"] == control.EMBEDDED_MIRI_FULL
    ]
    if len(specifications) != 1 or specifications[0].get("shards") != shard.total:
        raise ShardError("Miri shard total differs from the registered full suite")
    try:
        scenarios = miri.load_inventory()
        artifacts = miri.artifact_directory()
        miri.clear_owned_artifacts(scenarios, artifacts)
        miri.clear_owned_artifacts(scenarios, artifacts.parent / control.EMBEDDED_MIRI_FULL)
        identity = miri.prepare_identity()
    except miri.EmbeddedMiriError as error:
        raise ShardError(str(error)) from error
    environment = {
        "toolchain": asdict(identity),
        "host": {"platform": control.native_platform(), "machine": platform.machine()},
        "build_flags": {name: os.environ.get(name) for name in BUILD_FLAGS},
    }
    models = miri.mode_configuration(miri.Mode.FULL).borrow_models
    failures = []
    for scenario in scenarios:
        with ThreadPoolExecutor(max_workers=len(models)) as executor:
            pending = {
                model: executor.submit(
                    miri.execute_full_scenario, scenario, model, identity, artifacts, shard,
                    workers=SHARD_WORKERS_PER_MODEL,
                )
                for model in models
            }
            for model, future in pending.items():
                try:
                    execution = future.result()
                    clean_commit(commit)
                    write_fragment(
                        artifacts / f"{scenario.component}-{model.value}.shard.json",
                        {**constant_context(scenario, model, commit), **environment},
                        shard, execution.inventory, execution.results,
                    )
                except ProofExecutionError as error:
                    failures.append(error.failure.diagnostic)
                except (miri.EmbeddedMiriError, ShardError, OSError, subprocess.SubprocessError) as error:
                    failures.append(str(error))
    clean_commit(commit)
    if failures:
        raise ShardError("\n".join(failures))
    print(
        f"EMBEDDED_MIRI_SHARD_OK shard={shard.index}/{shard.total} "
        f"scenarios={len(scenarios)} models={len(models)}"
    )


def execution_environment(context: dict, expected: dict) -> dict:
    if set(context) != set(expected) | {"toolchain", "host", "build_flags"}:
        raise ShardError("Miri shard context has unexpected fields")
    toolchain = context["toolchain"]
    if (
        not isinstance(toolchain, dict)
        or set(toolchain) != {"channel", "rustc_version", "miri_version"}
        or any(not isinstance(value, str) or not value for value in toolchain.values())
        or toolchain["channel"] != miri.nightly_toolchain()
    ):
        raise ShardError("Miri shard does not identify the pinned toolchain")
    host = context["host"]
    if (
        not isinstance(host, dict) or set(host) != {"platform", "machine"}
        or not isinstance(host["platform"], str)
        or host["platform"] not in {"linux", "macos", "windows"}
        or not isinstance(host["machine"], str) or not host["machine"]
    ):
        raise ShardError("Miri shard host identity is invalid")
    flags = context["build_flags"]
    if (
        not isinstance(flags, dict) or set(flags) != set(BUILD_FLAGS)
        or any(value is not None and not isinstance(value, str) for value in flags.values())
    ):
        raise ShardError("Miri shard build flags are invalid")
    return {key: context[key] for key in ("toolchain", "host", "build_flags")}


def collect_proofs(suites: list[dict], artifact_root: Path, expected_sha: str) -> None:
    clean_commit(expected_sha)
    try:
        _collect_proofs(suites, artifact_root.resolve(), expected_sha)
    except miri.EmbeddedMiriError as error:
        raise ShardError(str(error)) from error


def _collect_proofs(suites: list[dict], artifact_root: Path, expected_sha: str) -> None:
    scenarios = miri.load_inventory()
    output = artifact_root / "results" / control.EMBEDDED_MIRI_FULL
    miri.clear_owned_artifacts(scenarios, output)
    ordered = sorted(suites, key=lambda suite: suite["shard"]["index"])
    directories = []
    run_records = []
    for index, suite in enumerate(ordered):
        expected_shard = {"suite": control.EMBEDDED_MIRI_FULL, **Shard(index, len(suites)).document()}
        if suite.get("shard") != expected_shard:
            raise ShardError("Miri aggregation requires every registered shard exactly once")
        directory = artifact_root / "results" / suite["id"]
        if artifact_root not in directory.resolve().parents:
            raise ShardError("Miri shard artifacts escape the artifact root")
        try:
            result = json.loads((directory / "result.json").read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ShardError(f"cannot read Miri validation result for {suite['id']}: {error}") from error
        errors = control.evidence_errors(result)
        if errors:
            raise ShardError(f"invalid Miri validation result for {suite['id']}: {'; '.join(errors)}")
        if (
            result["status"] != "passed" or result["worktree_clean"] is not True
            or result["commit"] != expected_sha or result["suite"] != suite["id"]
            or result["domain"] != suite["domain"] or result.get("shard") != expected_shard
            or result["required_platform"] != suite["platform"]
            or result["command"][1:] != control.command_for(suite, 0, directory)[1:]
        ):
            raise ShardError(f"Miri shard validation did not pass its registered contract: {suite['id']}")
        directories.append(directory)
        run_records.append(result)
    configuration = miri.mode_configuration(miri.Mode.FULL)
    shared_environment = None
    collected = []
    for scenario in scenarios:
        observations = []
        full_inventory = None
        for model in configuration.borrow_models:
            expected = constant_context(scenario, model, expected_sha)
            evidence = collect_model(tuple(directories), scenario.component, model.value, expected)
            environment = execution_environment(evidence.context, expected)
            if any(record["platform"] != environment["host"]["platform"] for record in run_records):
                raise ShardError("Miri shard host differs from its validation result")
            if any(
                record["tool_versions"].get("rustc-nightly") != environment["toolchain"]["rustc_version"]
                for record in run_records
            ):
                raise ShardError("Miri compiler differs from its validation result")
            if shared_environment is None:
                shared_environment = environment
            if environment != shared_environment:
                raise ShardError("Miri shards mix toolchains, hosts, or build flags")
            if full_inventory is None:
                full_inventory = evidence.inventory
            if evidence.inventory != full_inventory:
                raise ShardError("Miri borrow models discovered different full inventories")
            observations.append((model, evidence))
        collected.append((scenario, observations))
    if shared_environment is None:
        raise ShardError("Miri aggregation has no complete observations")
    clean_commit(expected_sha)
    identity = miri.ToolchainIdentity(**shared_environment["toolchain"])
    output.mkdir(parents=True, exist_ok=True)
    for scenario, evidence_by_model in collected:
        observations = []
        for model, evidence in evidence_by_model:
            log = output / f"{scenario.component}-{model.value}.log"
            headers = [
                ("discovered-tests=" + json.dumps(evidence.inventory) + "\n").encode(),
                ("validation-shards=" + json.dumps(run_records, sort_keys=True) + "\n").encode(),
                ("miri-shards=" + json.dumps(evidence.fragments, sort_keys=True) + "\n").encode(),
            ]
            log.write_bytes(miri.render_log(model, identity, [*headers, *evidence.outputs]))
            observations.append(miri.Observation(model, len(evidence.inventory), log))
        miri.record_proof(scenario, configuration, tuple(observations), output, identity)
    print(f"EMBEDDED_MIRI_PROOFS {output}")
