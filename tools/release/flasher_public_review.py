"""Immutable evidence for the exact protected flasher public-review gate."""

from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path


WORKFLOW_PATH = ".github/workflows/flasher-sign.yml"
JOB_NAME = "Approve protected public release"
EVIDENCE_FIELDS = {
    "schema",
    "repository",
    "workflow_path",
    "workflow_sha",
    "workflow_run_id",
    "workflow_run_attempt",
    "workflow_job_id",
    "version",
    "source_commit",
    "signed_candidate_sha256",
    "manifest_sha256",
    "prerelease_published_at",
    "approved_at",
}


def evidence_asset_name(*, version: str, run_id: int, run_attempt: int) -> str:
    validate_version(version)
    run_id = require_positive(run_id, "public-review workflow run ID")
    run_attempt = require_positive(
        run_attempt, "public-review workflow run attempt"
    )
    return f"public-review-v{version}-run-{run_id}-attempt-{run_attempt}.json"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def parse_time(value: object, label: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} must be an ISO UTC timestamp")
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValueError(f"{label} must be an ISO UTC timestamp") from error
    if parsed.tzinfo is None:
        raise ValueError(f"{label} must include a timezone")
    return parsed.astimezone(timezone.utc)


def canonical_time(value: datetime) -> str:
    return value.astimezone(timezone.utc).replace(microsecond=0).isoformat().replace(
        "+00:00", "Z"
    )


def require_sha256(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise ValueError(f"{label} must be a lowercase SHA-256")
    return value


def require_commit(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 40 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise ValueError(f"{label} must be a lowercase full Git commit")
    return value


def require_positive(value: object, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


def validate_version(value: str) -> str:
    if (
        not value
        or value.lower() == "next"
        or any(
            character
            not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+"
            for character in value
        )
    ):
        raise ValueError("public-review version is not immutable")
    return value


def release_identity(
    release: dict,
    *,
    version: str,
    source_commit: str,
    allow_promoted: bool = False,
) -> datetime:
    is_prerelease = release.get("isPrerelease")
    if release.get("isDraft") is not False or (
        is_prerelease is not True
        and not (allow_promoted and is_prerelease is False)
    ):
        raise ValueError("public review requires a visible non-draft prerelease")
    if release.get("tagName") != f"v{version}":
        raise ValueError("public prerelease tag differs from the reviewed version")
    if release.get("targetCommitish") != source_commit:
        raise ValueError("public prerelease source differs from the signed candidate")
    return parse_time(release.get("publishedAt"), "prerelease publishedAt")


def run_identity(
    run: dict, *, repository: str, run_id: int, run_attempt: int, workflow_sha: str
) -> None:
    if run.get("id") != run_id or run.get("run_attempt") != run_attempt:
        raise ValueError("public-review workflow run identity differs from the evidence")
    if run.get("path") != WORKFLOW_PATH or run.get("event") != "workflow_dispatch":
        raise ValueError("public-review evidence was not produced by the signing workflow")
    if run.get("head_sha") != workflow_sha:
        raise ValueError("public-review workflow revision differs from the signed source")
    run_repository = run.get("repository")
    if not isinstance(run_repository, dict) or run_repository.get("full_name") != repository:
        raise ValueError("public-review workflow repository differs from the release repository")


def job_identity(
    job: dict,
    *,
    run_id: int,
    run_attempt: int,
    job_id: int,
    workflow_sha: str,
) -> datetime:
    if (
        job.get("id") != job_id
        or job.get("run_id") != run_id
        or job.get("run_attempt") != run_attempt
    ):
        raise ValueError("public-review workflow job identity differs from the evidence")
    if job.get("name") != JOB_NAME or job.get("head_sha") != workflow_sha:
        raise ValueError("public-review workflow job name or revision differs")
    return parse_time(job.get("started_at"), "public-review job started_at")


def build_evidence(
    *,
    release: dict,
    run: dict,
    job: dict,
    signed_bundle: Path,
    manifest: Path,
    repository: str,
    version: str,
    source_commit: str,
    approved_at: str,
    allow_promoted: bool = False,
) -> dict:
    validate_version(version)
    require_commit(source_commit, "public-review source commit")
    run_id = require_positive(run.get("id"), "public-review workflow run ID")
    run_attempt = require_positive(
        run.get("run_attempt"), "public-review workflow run attempt"
    )
    workflow_sha = require_commit(run.get("head_sha"), "public-review workflow SHA")
    if workflow_sha != source_commit:
        raise ValueError("public-review workflow SHA differs from the signed source commit")
    run_identity(
        run,
        repository=repository,
        run_id=run_id,
        run_attempt=run_attempt,
        workflow_sha=workflow_sha,
    )
    job_id = require_positive(job.get("id"), "public-review workflow job ID")
    started_at = job_identity(
        job,
        run_id=run_id,
        run_attempt=run_attempt,
        job_id=job_id,
        workflow_sha=workflow_sha,
    )
    published_at = release_identity(
        release,
        version=version,
        source_commit=source_commit,
        allow_promoted=allow_promoted,
    )
    approval = parse_time(approved_at, "public-review approval")
    if started_at < published_at or approval < published_at:
        raise ValueError("protected public-review approval predates prerelease publication")
    if approval < started_at:
        raise ValueError("public-review approval predates its protected job")
    return {
        "schema": 2,
        "repository": repository,
        "workflow_path": WORKFLOW_PATH,
        "workflow_sha": workflow_sha,
        "workflow_run_id": run_id,
        "workflow_run_attempt": run_attempt,
        "workflow_job_id": job_id,
        "version": version,
        "source_commit": source_commit,
        "signed_candidate_sha256": sha256(signed_bundle),
        "manifest_sha256": sha256(manifest),
        "prerelease_published_at": canonical_time(published_at),
        "approved_at": canonical_time(approval),
    }


def validate_evidence(
    evidence: dict,
    *,
    release: dict,
    run: dict,
    job: dict,
    signed_bundle: Path,
    manifest: Path,
    repository: str,
    version: str,
    source_commit: str,
    allow_promoted: bool = False,
) -> None:
    if set(evidence) != EVIDENCE_FIELDS or evidence.get("schema") != 2:
        raise ValueError("public-review evidence has an unsupported shape")
    run_id = require_positive(
        evidence.get("workflow_run_id"), "public-review workflow run ID"
    )
    run_attempt = require_positive(
        evidence.get("workflow_run_attempt"),
        "public-review workflow run attempt",
    )
    workflow_sha = require_commit(
        evidence.get("workflow_sha"), "public-review workflow SHA"
    )
    run_identity(
        run,
        repository=repository,
        run_id=run_id,
        run_attempt=run_attempt,
        workflow_sha=workflow_sha,
    )
    job_identity(
        job,
        run_id=run_id,
        run_attempt=run_attempt,
        job_id=require_positive(
            evidence.get("workflow_job_id"), "public-review workflow job ID"
        ),
        workflow_sha=workflow_sha,
    )
    expected = build_evidence(
        release=release,
        run=run,
        job=job,
        signed_bundle=signed_bundle,
        manifest=manifest,
        repository=repository,
        version=version,
        source_commit=source_commit,
        approved_at=str(evidence.get("approved_at")),
        allow_promoted=allow_promoted,
    )
    if evidence != expected:
        raise ValueError("public-review evidence differs from the exact signed prerelease")
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        raise ValueError("public-review signing workflow did not complete successfully")
    if job.get("status") != "completed" or job.get("conclusion") != "success":
        raise ValueError("protected public-review job did not complete successfully")
    completed_at = parse_time(job.get("completed_at"), "public-review job completed_at")
    evidence_approval = parse_time(
        evidence.get("approved_at"), "public-review approval"
    )
    if evidence_approval > completed_at:
        raise ValueError("public-review approval lies outside its successful job")


def discover_evidence(
    directory: Path,
    *,
    repository: str,
    version: str,
    source_commit: str,
    workflow_run_id: int,
    signed_candidate_sha256: str,
    manifest_sha256: str,
) -> list[Path]:
    validate_version(version)
    require_commit(source_commit, "public-review source commit")
    workflow_run_id = require_positive(
        workflow_run_id, "public-review workflow run ID"
    )
    signed_candidate_sha256 = require_sha256(
        signed_candidate_sha256, "signed candidate SHA-256"
    )
    manifest_sha256 = require_sha256(manifest_sha256, "manifest SHA-256")
    if not directory.is_dir():
        raise ValueError("public-review evidence directory is unavailable")

    prefix = f"public-review-v{version}-run-{workflow_run_id}-attempt-"
    paths = [path for path in directory.iterdir() if path.name.startswith(prefix)]
    candidates: list[tuple[int, Path]] = []
    attempts: set[int] = set()
    for path in paths:
        if not path.is_file() or path.is_symlink():
            raise ValueError("public-review evidence directory contains a non-file entry")
        evidence = load_object(path, "public-review evidence")
        if set(evidence) != EVIDENCE_FIELDS or evidence.get("schema") != 2:
            raise ValueError("public-review evidence has an unsupported shape")
        run_attempt = require_positive(
            evidence.get("workflow_run_attempt"),
            "public-review workflow run attempt",
        )
        require_positive(evidence.get("workflow_job_id"), "public-review workflow job ID")
        expected_name = evidence_asset_name(
            version=version,
            run_id=workflow_run_id,
            run_attempt=run_attempt,
        )
        if path.name != expected_name:
            raise ValueError(
                f"public-review evidence asset must be named {expected_name}"
            )
        expected_identity = {
            "repository": repository,
            "workflow_path": WORKFLOW_PATH,
            "workflow_sha": source_commit,
            "workflow_run_id": workflow_run_id,
            "version": version,
            "source_commit": source_commit,
            "signed_candidate_sha256": signed_candidate_sha256,
            "manifest_sha256": manifest_sha256,
        }
        if any(evidence.get(field) != value for field, value in expected_identity.items()):
            raise ValueError(
                "public-review evidence asset differs from the signed release identity"
            )
        parse_time(evidence.get("prerelease_published_at"), "prerelease publishedAt")
        parse_time(evidence.get("approved_at"), "public-review approval")
        if run_attempt in attempts:
            raise ValueError("public-review evidence repeats a workflow run attempt")
        attempts.add(run_attempt)
        candidates.append((run_attempt, path))
    if not candidates:
        raise ValueError("no persistent public-review evidence assets were found")
    return [path for _, path in sorted(candidates, key=lambda candidate: candidate[0])]


def write_evidence(path: Path, evidence: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
