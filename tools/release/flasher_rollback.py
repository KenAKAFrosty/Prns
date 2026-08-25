"""Fail-closed rollback staging, compare-and-swap, and dry-run evidence."""

from __future__ import annotations

from datetime import datetime, timezone
import json
from pathlib import Path
from pathlib import PurePosixPath
import shutil
import tempfile

from flasher_manifest import FLASH_MANIFEST_SCHEMA
from urllib.parse import quote, urlsplit, urlunsplit
from urllib.request import Request, urlopen

from flasher_website_history import (
    canonical_version,
    require_commit,
    require_sha256,
    sha256,
    tree_files,
    tree_identity,
)


WORKFLOW_PATH = ".github/workflows/flasher-rollback.yml"
STAGE_JOB_NAME = "Verify and stage complete prior website"
STABLE_RELEASE = "StableRelease"
COMING_SOON = "ComingSoon"
TARGET_KINDS = {STABLE_RELEASE, COMING_SOON}
COMING_SOON_FILES = {
    "index.html": "docs/website/coming-soon/index.html",
    "404.html": "docs/website/coming-soon/index.html",
    "CNAME": "docs/website/public/CNAME",
    "assets/favicon.svg": "docs/website/public/assets/favicon.svg",
    "assets/prns-mark.svg": "docs/website/public/assets/prns-mark.svg",
    "assets/og.png": "docs/website/public/assets/og.png",
}


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def parse_time(value: object, label: str) -> datetime:
    if not isinstance(value, str):
        raise ValueError(f"{label} is missing")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError(f"{label} is malformed") from error
    if parsed.tzinfo is None:
        raise ValueError(f"{label} has no timezone")
    return parsed.astimezone(timezone.utc)


def validate_descriptor(path: Path, version: str, manifest_sha256: str) -> dict:
    version = canonical_version(version)
    manifest_sha256 = require_sha256(manifest_sha256, "expected live manifest SHA-256")
    descriptor = load_object(path, "stable channel descriptor")
    expected = {
        "schema": 1,
        "channel": "stable",
        "version": version,
        "manifest_url": f"https://reticulum.rs/releases/{version}/flash-manifest.json",
        "manifest_sha256": manifest_sha256,
    }
    if descriptor != expected:
        raise ValueError("live stable channel differs from the expected compare-and-swap identity")
    return descriptor


def validate_coming_soon(path: Path, canonical_index: Path) -> None:
    if (
        path.is_symlink()
        or canonical_index.is_symlink()
        or not path.is_file()
        or not canonical_index.is_file()
        or path.read_bytes() != canonical_index.read_bytes()
    ):
        raise ValueError("live site differs from the canonical coming-soon bytes")


def validate_live_state(
    path: Path,
    *,
    mode: str,
    target_kind: str,
    target_version: str | None,
    target_manifest_sha256: str | None,
    expected_live_version: str,
    expected_live_manifest_sha256: str,
    coming_soon_index: Path | None = None,
) -> str:
    """Validate the distinct pre-promotion dry-run and deployment CAS states."""

    if mode == "dry-run":
        try:
            if target_kind == STABLE_RELEASE:
                if target_version is None or target_manifest_sha256 is None:
                    raise ValueError("stable release target identity is incomplete")
                validate_descriptor(path, target_version, target_manifest_sha256)
            elif target_kind == COMING_SOON and coming_soon_index is not None:
                validate_coming_soon(path, coming_soon_index)
            else:
                raise ValueError("rollback target kind is unsupported")
            return "target_baseline"
        except ValueError:
            try:
                validate_descriptor(
                    path, expected_live_version, expected_live_manifest_sha256
                )
                return "expected_live"
            except ValueError as error:
                raise ValueError(
                    "dry-run live stable channel is neither the pre-promotion baseline "
                    "nor the expected operational release"
                ) from error
    if mode != "deploy":
        raise ValueError("rollback mode must be dry-run or deploy")
    try:
        validate_descriptor(
            path, expected_live_version, expected_live_manifest_sha256
        )
        return "expected_live"
    except ValueError:
        try:
            if target_kind == STABLE_RELEASE:
                if target_version is None or target_manifest_sha256 is None:
                    raise ValueError("stable release target identity is incomplete")
                validate_descriptor(path, target_version, target_manifest_sha256)
            elif target_kind == COMING_SOON and coming_soon_index is not None:
                validate_coming_soon(path, coming_soon_index)
            else:
                raise ValueError("rollback target kind is unsupported")
            return "target_idempotent_resume"
        except ValueError as target_error:
            raise ValueError(
                "live stable channel is neither the expected release nor the exact "
                "idempotent rollback target"
            ) from target_error


def validate_promotion_state(
    path: Path,
    *,
    baseline_kind: str,
    baseline_version: str | None,
    baseline_manifest_sha256: str | None,
    candidate_version: str,
    candidate_manifest_sha256: str,
    coming_soon_index: Path | None = None,
) -> str:
    try:
        validate_descriptor(path, candidate_version, candidate_manifest_sha256)
        return "candidate_idempotent_resume"
    except ValueError:
        if baseline_kind == STABLE_RELEASE:
            if baseline_version is None or baseline_manifest_sha256 is None:
                raise ValueError("stable release promotion baseline is incomplete")
            validate_descriptor(path, baseline_version, baseline_manifest_sha256)
        elif baseline_kind == COMING_SOON and coming_soon_index is not None:
            validate_coming_soon(path, coming_soon_index)
        else:
            raise ValueError("promotion baseline kind is unsupported")
        return "baseline"


def require_empty_directory(path: Path, label: str) -> None:
    if path.exists():
        if not path.is_dir() or any(path.iterdir()):
            raise ValueError(f"{label} must be a new or empty directory")
    path.mkdir(parents=True, exist_ok=True)


def validate_website_identity(value: object) -> dict:
    if not isinstance(value, dict) or set(value) != {
        "file_count",
        "total_bytes",
        "tree_sha256",
        "files",
    }:
        raise ValueError("rollback website identity has an unsupported shape")
    files = value.get("files")
    if not isinstance(files, list):
        raise ValueError("rollback website file inventory is unavailable")
    paths = set()
    normalized = []
    for item in files:
        if not isinstance(item, dict) or set(item) != {"path", "size", "sha256"}:
            raise ValueError("rollback website file identity is malformed")
        relative = item.get("path")
        size = item.get("size")
        checksum = item.get("sha256")
        if not isinstance(relative, str) or "\\" in relative:
            raise ValueError("rollback website file path is malformed")
        pure = PurePosixPath(relative)
        if (
            pure.is_absolute()
            or not pure.parts
            or any(part in {"", ".", ".."} for part in pure.parts)
            or relative != pure.as_posix()
            or relative in paths
        ):
            raise ValueError("rollback website file path is unsafe or duplicated")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ValueError("rollback website file size is invalid")
        require_sha256(checksum, "rollback website file SHA-256")
        paths.add(relative)
        normalized.append(item)
    if normalized != sorted(normalized, key=lambda item: str(item["path"])):
        raise ValueError("rollback website file inventory is not canonical")
    expected_tree = tree_identity(normalized)
    if any(value.get(field) != expected_tree[field] for field in expected_tree):
        raise ValueError("rollback website tree identity differs from its file inventory")
    return value


def validate_stage_identity(value: object) -> dict:
    if not isinstance(value, dict) or set(value) != {"schema", "target", "website"}:
        raise ValueError("rollback stage identity has an unsupported shape")
    if value.get("schema") != 3:
        raise ValueError("rollback stage identity has an unsupported schema")
    target = value.get("target")
    if not isinstance(target, dict):
        raise ValueError("rollback target identity is malformed")
    kind = target.get("kind")
    if kind == STABLE_RELEASE:
        expected_target_fields = {
            "kind",
            "version",
            "source_commit",
            "manifest_sha256",
            "release_record_sha256",
        }
        if set(target) != expected_target_fields:
            raise ValueError("stable rollback target identity is malformed")
        canonical_version(target.get("version"))
        require_commit(target.get("source_commit"), "rollback target source commit")
        require_sha256(target.get("manifest_sha256"), "rollback target manifest SHA-256")
        require_sha256(
            target.get("release_record_sha256"),
            "rollback target release-record SHA-256",
        )
    elif kind == COMING_SOON:
        if set(target) != {
            "kind",
            "withdrawn_version",
            "withdrawn_manifest_sha256",
        }:
            raise ValueError("coming-soon rollback target identity is malformed")
        canonical_version(target.get("withdrawn_version"))
        require_sha256(
            target.get("withdrawn_manifest_sha256"),
            "withdrawn manifest SHA-256",
        )
    else:
        raise ValueError("rollback target kind is unsupported")
    validate_website_identity(value.get("website"))
    return value


def verify_live_website(*, stage_identity: Path, site_url: str) -> dict:
    identity = validate_stage_identity(load_object(stage_identity, "rollback stage identity"))
    parsed = urlsplit(site_url)
    if (
        parsed.scheme not in {"http", "https"}
        or not parsed.netloc
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError("rollback website URL must be an absolute HTTP(S) origin or path")
    root_path = parsed.path.rstrip("/")
    website = identity["website"]
    with tempfile.TemporaryDirectory(prefix="prns-rollback-live-") as temporary:
        root = Path(temporary)
        for item in website["files"]:
            relative = str(item["path"])
            remote_path = f"{root_path}/{quote(relative, safe='/')}"
            url = urlunsplit(
                (
                    parsed.scheme,
                    parsed.netloc,
                    remote_path,
                    f"prns_tree={website['tree_sha256']}",
                    "",
                )
            )
            request = Request(url, headers={"Cache-Control": "no-cache"})
            with urlopen(request, timeout=30) as response:
                if response.status != 200:
                    raise ValueError(
                        f"rollback website fetch returned HTTP {response.status}: {relative}"
                    )
                contents = response.read(int(item["size"]) + 1)
            if len(contents) != item["size"]:
                raise ValueError(f"deployed rollback website size differs: {relative}")
            destination = root.joinpath(*PurePosixPath(relative).parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(contents)
            if destination.stat().st_size != item["size"] or sha256(destination) != item["sha256"]:
                raise ValueError(f"deployed rollback website bytes differ: {relative}")
        actual_files = tree_files(root)
        if actual_files != website["files"] or tree_identity(actual_files) != {
            field: website[field]
            for field in ("file_count", "total_bytes", "tree_sha256")
        }:
            raise ValueError("deployed rollback website tree differs from the staged tree")
    return website


def stage(
    *,
    candidate: Path,
    release_record: Path,
    release_record_sha256: str,
    version: str,
    output: Path,
    identity_output: Path,
) -> dict:
    version = canonical_version(version)
    expected_record_hash = require_sha256(
        release_record_sha256, "target release-record SHA-256"
    )
    if sha256(release_record) != expected_record_hash:
        raise ValueError("target release record differs from the operator-supplied SHA-256")
    manifest = load_object(candidate / "flash-manifest.json", "target manifest")
    release = manifest.get("release")
    manifest_schema = manifest.get("schema")
    if (
        not isinstance(manifest_schema, int)
        or isinstance(manifest_schema, bool)
        or manifest_schema < 1
        or manifest_schema > FLASH_MANIFEST_SCHEMA
        or not isinstance(release, dict)
        or release.get("version") != version
        or release.get("channel") != "stable"
    ):
        raise ValueError("rollback target is not the exact signed stable candidate")
    manifest_hash = sha256(candidate / "flash-manifest.json")
    record = load_object(release_record, "target release record")
    record_release = record.get("release")
    record_candidate = record.get("candidate")
    record_manifest = (
        record_candidate.get("manifest") if isinstance(record_candidate, dict) else None
    )
    if (
        not isinstance(record_release, dict)
        or record_release.get("version") != version
        or record_release.get("channel") != "stable"
        or not isinstance(record_manifest, dict)
        or record_manifest.get("sha256") != manifest_hash
    ):
        raise ValueError("target release record differs from the signed rollback candidate")
    source_commit = require_commit(
        record_release.get("source_commit"), "rollback target source commit"
    )
    if release.get("commit") != source_commit:
        raise ValueError("rollback target source commit differs from its release record")
    descriptor = candidate / "website" / "releases" / "channels" / "stable.json"
    validate_descriptor(descriptor, version, manifest_hash)
    if not Path(f"{descriptor}.minisig").is_file():
        raise ValueError("rollback target website lacks the signed stable descriptor")
    website = candidate / "website"
    if website.is_symlink() or not website.is_dir() or not (website / "index.html").is_file():
        raise ValueError("rollback target website is incomplete")
    require_empty_directory(output, "rollback staging output")
    shutil.copytree(
        website,
        output,
        dirs_exist_ok=True,
        symlinks=True,
        copy_function=shutil.copy2,
    )
    files = tree_files(output)
    identity = {
        "schema": 3,
        "target": {
            "kind": STABLE_RELEASE,
            "version": version,
            "source_commit": source_commit,
            "manifest_sha256": manifest_hash,
            "release_record_sha256": expected_record_hash,
        },
        "website": {**tree_identity(files), "files": files},
    }
    identity_output.parent.mkdir(parents=True, exist_ok=True)
    identity_output.write_text(
        json.dumps(identity, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return identity


def stage_coming_soon(
    *,
    repository: Path,
    withdrawn_version: str,
    withdrawn_manifest_sha256: str,
    output: Path,
    identity_output: Path,
) -> dict:
    withdrawn_version = canonical_version(withdrawn_version)
    withdrawn_manifest_sha256 = require_sha256(
        withdrawn_manifest_sha256,
        "withdrawn manifest SHA-256",
    )
    require_empty_directory(output, "rollback staging output")
    for destination, source in COMING_SOON_FILES.items():
        source_path = repository / source
        if source_path.is_symlink() or not source_path.is_file():
            raise ValueError(f"canonical coming-soon source is unavailable: {source}")
        destination_path = output.joinpath(*PurePosixPath(destination).parts)
        destination_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source_path, destination_path)
    files = tree_files(output)
    identity = {
        "schema": 3,
        "target": {
            "kind": COMING_SOON,
            "withdrawn_version": withdrawn_version,
            "withdrawn_manifest_sha256": withdrawn_manifest_sha256,
        },
        "website": {**tree_identity(files), "files": files},
    }
    identity_output.parent.mkdir(parents=True, exist_ok=True)
    identity_output.write_text(
        json.dumps(identity, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return identity


def create_dry_run_record(
    *,
    stage_identity: Path,
    expected_live_version: str,
    expected_live_manifest_sha256: str,
    repository: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    workflow_job_id: int,
    workflow_sha: str,
    observed_live_state: str,
    started_epoch: int,
    output: Path,
    now: datetime | None = None,
) -> dict:
    identity = load_object(stage_identity, "rollback stage identity")
    validate_stage_identity(identity)
    canonical_version(expected_live_version)
    require_sha256(expected_live_manifest_sha256, "expected live manifest SHA-256")
    require_commit(workflow_sha, "rollback workflow SHA")
    if workflow_run_id <= 0 or workflow_run_attempt <= 0 or workflow_job_id <= 0:
        raise ValueError("rollback workflow run identity must be positive")
    if observed_live_state not in {"target_baseline", "expected_live"}:
        raise ValueError("rollback dry-run observed live state is unsupported")
    current = now or datetime.now(timezone.utc)
    elapsed = int(current.timestamp()) - started_epoch
    if started_epoch <= 0 or elapsed < 0 or elapsed > 900:
        raise ValueError("rollback dry-run staging exceeded the 15-minute recovery target")
    record = {
        "schema": 3,
        "result": "passed",
        "deployment_cas": "deferred_to_deploy",
        "repository": repository,
        "workflow_path": WORKFLOW_PATH,
        "workflow_run_id": workflow_run_id,
        "workflow_run_attempt": workflow_run_attempt,
        "workflow_job_id": workflow_job_id,
        "workflow_sha": workflow_sha,
        "observed_live_state": observed_live_state,
        "target": identity["target"],
        "expected_live": {
            "version": expected_live_version,
            "manifest_sha256": expected_live_manifest_sha256,
        },
        "staged_website": identity["website"],
        "elapsed_seconds": elapsed,
        "measured_started_at_utc": datetime.fromtimestamp(
            started_epoch, timezone.utc
        ).replace(microsecond=0).isoformat(),
        "completed_at_utc": current.replace(microsecond=0).isoformat(),
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return record


def validate_dry_run_record(
    *,
    record_path: Path,
    run_json: Path,
    job_json: Path,
    stage_identity: Path,
    repository: str,
    default_branch: str,
    expected_run_id: int,
    expected_run_attempt: int,
    target_kind: str,
    target_version: str | None,
    target_release_record_sha256: str | None,
    expected_live_version: str,
    expected_live_manifest_sha256: str,
    required_workflow_sha: str,
    required_observed_live_state: str | None = None,
) -> dict:
    record = load_object(record_path, "rollback dry-run record")
    run = load_object(run_json, "rollback dry-run workflow run")
    job = load_object(job_json, "rollback dry-run workflow job")
    identity = load_object(stage_identity, "rollback stage identity")
    validate_stage_identity(identity)
    expected_fields = {
        "schema",
        "result",
        "deployment_cas",
        "repository",
        "workflow_path",
        "workflow_run_id",
        "workflow_run_attempt",
        "workflow_job_id",
        "workflow_sha",
        "observed_live_state",
        "target",
        "expected_live",
        "staged_website",
        "elapsed_seconds",
        "measured_started_at_utc",
        "completed_at_utc",
    }
    if (
        set(record) != expected_fields
        or record.get("schema") != 3
        or record.get("result") != "passed"
        or record.get("deployment_cas") != "deferred_to_deploy"
    ):
        raise ValueError("rollback dry-run record has an unsupported shape or result")
    expected_live_version = canonical_version(expected_live_version)
    live_hash = require_sha256(
        expected_live_manifest_sha256, "expected live manifest SHA-256"
    )
    required_workflow_sha = require_commit(
        required_workflow_sha, "required rollback workflow SHA"
    )
    if target_kind not in TARGET_KINDS:
        raise ValueError("rollback target kind is unsupported")
    if record.get("repository") != repository or record.get("workflow_path") != WORKFLOW_PATH:
        raise ValueError("rollback dry-run record has the wrong workflow custody")
    if record.get("workflow_run_id") != expected_run_id:
        raise ValueError("rollback dry-run record has the wrong workflow run ID")
    if (
        not isinstance(expected_run_attempt, int)
        or isinstance(expected_run_attempt, bool)
        or expected_run_attempt <= 0
        or record.get("workflow_run_attempt") != expected_run_attempt
    ):
        raise ValueError("rollback dry-run record has the wrong workflow run attempt")
    observed_live_state = record.get("observed_live_state")
    if observed_live_state not in {"target_baseline", "expected_live"}:
        raise ValueError("rollback dry-run record has an unsupported observed live state")
    if (
        required_observed_live_state is not None
        and observed_live_state != required_observed_live_state
    ):
        raise ValueError("rollback dry-run record observed the wrong live release state")
    if record.get("workflow_sha") != required_workflow_sha:
        raise ValueError("rollback dry-run record was produced by a different workflow revision")
    if record.get("target") != identity.get("target") or record.get("staged_website") != identity.get("website"):
        raise ValueError("rollback dry-run record differs from the newly staged website")
    target = record.get("target")
    if not isinstance(target, dict) or target.get("kind") != target_kind:
        raise ValueError("rollback dry-run record has the wrong target kind")
    if target_kind == STABLE_RELEASE:
        if target_version is None or target_release_record_sha256 is None:
            raise ValueError("stable release target identity is incomplete")
        target_version = canonical_version(target_version)
        target_hash = require_sha256(
            target_release_record_sha256, "target release-record SHA-256"
        )
        if target_version == expected_live_version:
            raise ValueError("rollback target must differ from the expected live release")
        if (
            target.get("version") != target_version
            or target.get("release_record_sha256") != target_hash
        ):
            raise ValueError("rollback dry-run record has the wrong target identity")
    elif (
        target_version is not None
        or target_release_record_sha256 is not None
        or target.get("withdrawn_version") != expected_live_version
        or target.get("withdrawn_manifest_sha256") != live_hash
    ):
        raise ValueError("coming-soon dry-run does not bind the exact withdrawn release")
    if record.get("expected_live") != {
        "version": expected_live_version,
        "manifest_sha256": live_hash,
    }:
        raise ValueError("rollback dry-run record has the wrong live compare-and-swap identity")
    elapsed = record.get("elapsed_seconds")
    if not isinstance(elapsed, int) or isinstance(elapsed, bool) or not 0 <= elapsed <= 900:
        raise ValueError("rollback dry-run record exceeded the 15-minute recovery target")
    completed = parse_time(record.get("completed_at_utc"), "rollback completion time")
    measured_started = parse_time(
        record.get("measured_started_at_utc"), "rollback measured start time"
    )
    if int((completed - measured_started).total_seconds()) != elapsed:
        raise ValueError("rollback dry-run measured time is internally inconsistent")
    allowed_paths = {WORKFLOW_PATH, f"{WORKFLOW_PATH}@refs/heads/{default_branch}"}
    repository_value = run.get("repository")
    head_repository = run.get("head_repository")
    checks = {
        "run ID": run.get("id") == expected_run_id,
        "repository": isinstance(repository_value, dict)
        and repository_value.get("full_name") == repository,
        "head repository": isinstance(head_repository, dict)
        and head_repository.get("full_name") == repository,
        "workflow path": run.get("path") in allowed_paths,
        "event": run.get("event") == "workflow_dispatch",
        "status": run.get("status") == "completed",
        "conclusion": run.get("conclusion") == "success",
        "default branch": run.get("head_branch") == default_branch,
        "workflow SHA": run.get("head_sha") == record.get("workflow_sha"),
        "run attempt": run.get("run_attempt") == record.get("workflow_run_attempt"),
    }
    failed = [name for name, passed in checks.items() if not passed]
    if failed:
        raise ValueError(f"rollback dry-run workflow custody checks failed: {failed}")
    run_started = parse_time(run.get("run_started_at"), "rollback workflow start time")
    run_updated = parse_time(run.get("updated_at"), "rollback workflow completion time")
    if run_updated < run_started:
        raise ValueError("rollback dry-run workflow timing is malformed")
    job_checks = {
        "job ID": job.get("id") == record.get("workflow_job_id"),
        "run ID": job.get("run_id") == expected_run_id,
        "name": job.get("name") == STAGE_JOB_NAME,
        "status": job.get("status") == "completed",
        "conclusion": job.get("conclusion") == "success",
        "workflow SHA": job.get("head_sha") == required_workflow_sha,
        "run attempt": job.get("run_attempt") == record.get("workflow_run_attempt"),
    }
    failed_jobs = [name for name, passed in job_checks.items() if not passed]
    if failed_jobs:
        raise ValueError(f"rollback dry-run job custody checks failed: {failed_jobs}")
    job_started = parse_time(job.get("started_at"), "rollback stage job start time")
    job_completed = parse_time(job.get("completed_at"), "rollback stage job completion time")
    if job_completed < job_started or (job_completed - job_started).total_seconds() > 900:
        raise ValueError("successful rollback dry-run stage job exceeded 15 minutes")
    if measured_started < job_started or completed > job_completed:
        raise ValueError("rollback dry-run measurement falls outside its successful stage job")
    if job_started < run_started or job_completed > run_updated:
        raise ValueError("rollback dry-run stage job falls outside its successful workflow run")
    if completed < measured_started:
        raise ValueError("rollback dry-run record time falls outside its successful workflow run")
    return record
