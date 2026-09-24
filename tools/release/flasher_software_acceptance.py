"""Exact-source automated acceptance for pre-1.0 embedded releases."""

from __future__ import annotations

from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import re
import tomllib

from flasher_acceptance_contract import (
    SOFTWARE_ACCEPTANCE_SCHEMA,
    sha256,
    software_qualification,
)


def readiness_timestamp(value: object, label: str) -> datetime:
    if not isinstance(value, str) or re.fullmatch(
        r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
        r"(?:\.[0-9]{1,6})?(?:Z|\+00:00)", value
    ) is None:
        raise ValueError(f"{label} must be a full UTC ISO-8601 timestamp")
    try:
        return datetime.fromisoformat(value)
    except ValueError as error:
        raise ValueError(f"{label} must be a valid UTC timestamp") from error


def validation_contract():
    directory = Path(__file__).resolve().parent
    runner_path = directory / "validation_runner.py"
    inventory_path = directory / "validation-manifest.toml"
    if not runner_path.is_file():
        runner_path = directory.parents[1] / "validation" / "run.py"
        inventory_path = directory.parents[1] / "validation" / "manifest.toml"
    spec = importlib.util.spec_from_file_location("flasher_validation_runner", runner_path)
    if spec is None or spec.loader is None:
        raise ValueError("candidate validation runner is missing")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    with inventory_path.open("rb") as stream:
        inventory = tomllib.load(stream)
    return runner, inventory


def readiness_errors(document: object, commit: str, now: datetime) -> list[str]:
    if not isinstance(document, dict):
        return ["automated evidence must be a release manifest object"]
    errors = []
    if document.get("schema") != 1 or document.get("tier") != "release":
        errors.append("automated evidence must use release-tier schema 1")
    if document.get("commit") != commit:
        errors.append("automated evidence differs from the exact candidate commit")
    runner, inventory = validation_contract()
    required = {suite["id"]: suite for suite in runner.selected_suites(inventory, [], None, "release")}
    results = document.get("results")
    if not isinstance(results, dict):
        return errors + ["automated evidence results must be an object"]
    if set(results) != set(required):
        errors.append(f"automated suite inventory differs: missing={sorted(set(required) - set(results))}, unexpected={sorted(set(results) - set(required))}")
    try:
        generated = readiness_timestamp(document.get("generated_at"), "automated evidence generated_at")
        if generated > now:
            errors.append("automated evidence generation cannot be in the future")
    except ValueError as error:
        errors.append(str(error))
        generated = now
    for name, suite in required.items():
        if name not in results:
            continue
        result = results[name]
        errors.extend(f"{name}: {error}" for error in runner.evidence_errors(result))
        if not isinstance(result, dict):
            continue
        if result.get("suite") != name or result.get("domain") != suite["domain"]:
            errors.append(f"{name}: result identity differs from the inventory")
        if result.get("commit") != commit or result.get("worktree_clean") is not True:
            errors.append(f"{name}: requires the exact clean candidate commit")
        if result.get("status") != "passed":
            errors.append(f"{name}: automated release gate did not pass")
        if result.get("required_platform") != suite["platform"] or (
            suite["platform"] != "any" and result.get("platform") != suite["platform"]
        ):
            errors.append(f"{name}: result platform differs from the inventory")
        try:
            finished = readiness_timestamp(result.get("finished_at"), f"{name} finished_at")
            if finished > generated:
                errors.append(f"{name}: result finishes after evidence generation")
        except ValueError as error:
            errors.append(str(error))
    return errors


def create_record(manifest: dict, arguments) -> dict:
    readiness = getattr(arguments, "readiness_manifest", None)
    run_id = getattr(arguments, "readiness_run_id", None)
    if readiness is None or not isinstance(run_id, str) or re.fullmatch(r"[1-9][0-9]*", run_id) is None:
        raise ValueError("automated acceptance requires --readiness-manifest and --readiness-run-id")
    release = manifest["release"]
    if not software_qualification(release.get("version")):
        raise ValueError("automated acceptance requires a pre-1.0 release from 0.3.8 onward")
    errors = readiness_errors(json.loads(readiness.read_text(encoding="utf-8")), release["commit"], datetime.now(timezone.utc))
    if errors:
        raise ValueError("; ".join(errors))
    digest = sha256(readiness)
    return {
        "schema": SOFTWARE_ACCEPTANCE_SCHEMA,
        "candidate": {
            "version": release["version"],
            "channel": release["channel"],
            "source_commit": release["commit"],
            "signing_key_id": manifest["signing"]["key_id"],
            "manifest_sha256": sha256(arguments.manifest),
            "manifest_signature_sha256": sha256(arguments.manifest_signature),
            "signed_candidate_sha256": sha256(arguments.signed_bundle),
            "prerelease_published_at": arguments.prerelease_published_at,
        },
        "software_validation": {
            "workflow_run_id": run_id,
            "reference": f"artifact://qualification/{digest}",
            "sha256": digest,
        },
        "physical_qualification": "not-required-pre-1.0",
    }


def validate_record(acceptance: dict, commit: str, version: str, store, now: datetime) -> list[str]:
    errors = []
    if set(acceptance) != {"schema", "candidate", "software_validation", "physical_qualification"}:
        errors.append("automated acceptance has missing or unexpected fields")
    if not software_qualification(version):
        errors.append("automated acceptance requires a pre-1.0 release from 0.3.8 onward")
    if acceptance.get("physical_qualification") != "not-required-pre-1.0":
        errors.append("automated acceptance must not claim physical qualification")
    evidence = acceptance.get("software_validation")
    if not isinstance(evidence, dict) or set(evidence) != {"workflow_run_id", "reference", "sha256"}:
        return errors + ["software_validation must identify one exact readiness run and evidence object"]
    if not isinstance(evidence["workflow_run_id"], str) or re.fullmatch(r"[1-9][0-9]*", evidence["workflow_run_id"]) is None:
        errors.append("software_validation workflow_run_id must be a positive decimal string")
    digest = evidence["sha256"]
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        return errors + ["software_validation sha256 is malformed"]
    if evidence["reference"] != f"artifact://qualification/{digest}":
        return errors + ["software_validation reference differs from its SHA-256"]
    store.referenced_digests.add(digest)
    path = store.root / digest
    if path.is_symlink() or not path.is_file() or sha256(path) != digest:
        return errors + ["automated evidence is missing or differs from its SHA-256"]
    try:
        errors.extend(readiness_errors(json.loads(path.read_text(encoding="utf-8")), commit, now))
    except (ValueError, OSError) as error:
        errors.append(f"automated evidence cannot be validated: {error}")
    store.validate_inventory(errors)
    return errors
