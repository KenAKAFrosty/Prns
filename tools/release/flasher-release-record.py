#!/usr/bin/env python3
"""Create or verify the signed record that binds a qualified flasher release."""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile

from flasher_acceptance_contract import parse_utc_timestamp
from flasher_public_review import (
    EVIDENCE_FIELDS,
    SUITE_WORKFLOW_PATH,
    WORKFLOW_PATH,
    WORKFLOW_PATHS,
    evidence_asset_name,
    require_commit as require_review_commit,
    require_positive,
    require_sha256 as require_review_sha256,
)
from flasher_release_evidence import attestation_subjects, sha256
from flasher_manifest import FLASH_MANIFEST_SCHEMA, target_artifacts


CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
FLASHER_CANDIDATE_WORKFLOW = ".github/workflows/flasher-candidate.yml"
V037_ARCHIVE_COVERAGE = {
    "version": "0.3.7",
    "source_commit": "95404f41675b4a38907af09253460aeea518e5f2",
    "signed_bundle_sha256": "5815c0e037e6eb76aee65aa53871accf542e7e3b5ce447bcf1f6a0d41952c0a4",
    "attestation_bundle_sha256": "744e21da34de9e525c4d607ea4c4f0641b3681ab5a69f96245dadeb87d029bad",
    "attestation_workflow_run_id": 32617066008,
    "subjects": frozenset(
        {
            (
                "firmware/hopspot/t-echo/0.3.7/t-echo-s140-6.1.1.uf2",
                "43b5daf111306078f4d67a9c38715be489583f0398f07ac93a3851c6a7913f38",
            ),
            (
                "firmware/hopspot/t-echo/0.3.7/t-echo-s140-7.3.0.uf2",
                "ec080887c1d053a20b72cadfd56a90f52699bde22b793ac98251bd32566bc856",
            ),
            (
                "firmware/hopspot/t096/0.3.7/t096-s140-6.1.1.uf2",
                "8ac1e8e15008cef68ad06a4aebbfa6e6298ee381da833745f9019dbffe72597b",
            ),
            (
                "firmware/hopspot/t1000-e/0.3.7/t1000e.bin",
                "89f9668b217321bca790a9f0aebe59f519c9396b9b6b3e350a7b250ec51da198",
            ),
            (
                "firmware/hopspot/t1000-e/0.3.7/t1000e.dat",
                "edbfd54cc967870f8a53f242d2a38119a0eb6d71fc7d74d0ac2d96d57e155408",
            ),
            (
                "firmware/hopspot/t1000-e/0.3.7/t1000e.uf2",
                "aedaf3498f68f8f86f996e5505da7283fb832481b71e58b4cb5fbf5fbb533736",
            ),
            (
                "firmware/hopspot/t114/0.3.7/heltec-t114-s140-6.1.1.uf2",
                "33b428b73e1994cfe76508d4c922a08f0d92b4885448ee4f9389eb440b3a5ecb",
            ),
        }
    ),
}


def file_identity(path: Path) -> dict[str, str | int]:
    if not path.is_file():
        raise ValueError(f"release evidence file is unavailable: {path}")
    return {"name": path.name, "size": path.stat().st_size, "sha256": sha256(path)}


def document_identity(document: Path) -> dict[str, str]:
    signature = Path(f"{document}.minisig")
    if not document.is_file() or not signature.is_file():
        raise ValueError(f"signed release document is incomplete: {document}")
    return {"sha256": sha256(document), "signature_sha256": sha256(signature)}


def safe_candidate_path(candidate: Path, relative: str) -> Path:
    pure = PurePosixPath(relative)
    if (
        "\\" in relative
        or pure.is_absolute()
        or not pure.parts
        or any(part in {"", ".", ".."} for part in pure.parts)
    ):
        raise ValueError(f"manifest firmware path is unsafe: {relative!r}")
    return candidate.joinpath(*pure.parts)


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def candidate_file_inventory(root: Path) -> list[dict[str, str | int]]:
    if not root.is_dir() or root.is_symlink():
        raise ValueError("candidate must be a regular directory")
    inventory = []
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"candidate contains a symlink: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"candidate contains an unsupported entry: {path}")
        inventory.append(
            {
                "path": path.relative_to(root).as_posix(),
                "size": path.stat().st_size,
                "sha256": sha256(path),
            }
        )
    return sorted(inventory, key=lambda item: str(item["path"]))


def bind_candidate_to_signed_archive(candidate: Path, signed_bundle: Path) -> None:
    extractor = Path(__file__).with_name("extract-flasher-candidate.py")
    with tempfile.TemporaryDirectory(prefix="prns-release-record-candidate-") as temporary:
        extracted = Path(temporary) / "candidate"
        subprocess.run(
            [sys.executable, str(extractor), str(signed_bundle), str(extracted)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        supplied_inventory = candidate_file_inventory(candidate)
        archive_inventory = candidate_file_inventory(extracted)
        if supplied_inventory != archive_inventory:
            raise ValueError(
                "candidate directory bytes differ from the exact signed candidate archive"
            )


def archive_coverage(
    *,
    version: str,
    source_commit: str,
    signed_bundle: dict,
    attestation_bundle_sha256: str,
    attestation_workflow_run_id: int,
    attested_subjects: set[tuple[str, str]],
    missing: set[tuple[str, str]],
    unexpected: set[tuple[str, str]],
) -> dict | None:
    exception = V037_ARCHIVE_COVERAGE
    identity_matches = (
        version == exception["version"]
        and source_commit == exception["source_commit"]
        and signed_bundle.get("sha256") == exception["signed_bundle_sha256"]
        and attestation_bundle_sha256 == exception["attestation_bundle_sha256"]
        and attestation_workflow_run_id == exception["attestation_workflow_run_id"]
    )
    if not identity_matches or unexpected or missing != exception["subjects"]:
        return None
    archive_identity = (str(signed_bundle.get("name")), str(signed_bundle.get("sha256")))
    if archive_identity not in attested_subjects:
        return None
    return {
        "schema": 1,
        "scope": "v0.3.7-nordic-attestation-enumeration",
        "protection": "exact-files-in-github-attested-signed-candidate",
        "subjects": [
            {"name": name, "sha256": checksum}
            for name, checksum in sorted(missing)
        ],
    }


def public_review_identity(
    path: Path,
    *,
    repository: str,
    version: str,
    source_commit: str,
    signed_bundle_sha256: str,
    manifest_sha256: str,
    prerelease_published_at: str,
) -> dict:
    evidence = load_object(path, "public-review evidence")
    if set(evidence) != EVIDENCE_FIELDS or evidence.get("schema") != 2:
        raise ValueError("public-review evidence has an unsupported shape")
    run_id = require_positive(
        evidence.get("workflow_run_id"), "public-review workflow run ID"
    )
    run_attempt = require_positive(
        evidence.get("workflow_run_attempt"), "public-review workflow run attempt"
    )
    job_id = require_positive(
        evidence.get("workflow_job_id"), "public-review workflow job ID"
    )
    expected_name = evidence_asset_name(
        version=version, run_id=run_id, run_attempt=run_attempt
    )
    if path.name != expected_name:
        raise ValueError(f"public-review evidence must be named {expected_name}")
    expected = {
        "repository": repository,
        "workflow_sha": source_commit,
        "version": version,
        "source_commit": source_commit,
        "signed_candidate_sha256": signed_bundle_sha256,
        "manifest_sha256": manifest_sha256,
        "prerelease_published_at": prerelease_published_at,
    }
    if any(evidence.get(field) != value for field, value in expected.items()):
        raise ValueError("public-review evidence differs from the exact signed release")
    workflow_path = evidence.get("workflow_path")
    if workflow_path not in WORKFLOW_PATHS:
        raise ValueError("public-review evidence names an unregistered release workflow")
    require_review_commit(evidence.get("workflow_sha"), "public-review workflow SHA")
    require_review_sha256(
        evidence.get("signed_candidate_sha256"), "public-review candidate SHA-256"
    )
    require_review_sha256(
        evidence.get("manifest_sha256"), "public-review manifest SHA-256"
    )
    parse_utc_timestamp(
        evidence.get("prerelease_published_at"), "public-review prerelease publishedAt"
    )
    parse_utc_timestamp(evidence.get("approved_at"), "public-review approval")
    return {
        "evidence": file_identity(path),
        "workflow_path": workflow_path,
        "workflow_sha": source_commit,
        "workflow_run_id": run_id,
        "workflow_run_attempt": run_attempt,
        "workflow_job_id": job_id,
        "approved_at": evidence["approved_at"],
    }


def require_commit(value: str, label: str) -> None:
    if len(value) != 40 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{label} must be a lowercase full Git commit")


def candidate_run_identity(
    path: Path, *, version: str, repository: str, source_commit: str
) -> dict:
    evidence = load_object(path, "candidate workflow run evidence")
    expected_fields = {
        "schema",
        "repository",
        "workflow_path",
        "workflow_run_id",
        "workflow_run_attempt",
        "source_commit",
    }
    if set(evidence) != expected_fields or evidence.get("schema") != 1:
        raise ValueError("candidate workflow run evidence has an unsupported shape")
    expected_name = f"prns-flasher-candidate-run-v{version}.json"
    if path.name != expected_name:
        raise ValueError(f"candidate workflow run evidence must be named {expected_name}")
    if evidence.get("repository") != repository:
        raise ValueError("candidate workflow run repository differs from the release repository")
    if evidence.get("workflow_path") != FLASHER_CANDIDATE_WORKFLOW:
        raise ValueError("candidate workflow run path is not the candidate builder")
    if evidence.get("source_commit") != source_commit:
        raise ValueError("candidate workflow run source commit differs from the signed manifest")
    for field in ("workflow_run_id", "workflow_run_attempt"):
        value = evidence.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            raise ValueError(f"candidate {field} must be a positive integer")
    return {
        "evidence": file_identity(path),
        "repository": evidence["repository"],
        "workflow_path": evidence["workflow_path"],
        "workflow_run_id": evidence["workflow_run_id"],
        "workflow_run_attempt": evidence["workflow_run_attempt"],
        "source_commit": evidence["source_commit"],
    }


def build_record(arguments: argparse.Namespace) -> dict:
    parse_utc_timestamp(arguments.prerelease_published_at, "prerelease publishedAt")
    candidate = arguments.candidate.resolve()
    manifest_path = candidate / "flash-manifest.json"
    manifest = load_object(manifest_path, "candidate manifest")
    release = manifest.get("release")
    signing = manifest.get("signing")
    if manifest.get("schema") != FLASH_MANIFEST_SCHEMA or not isinstance(release, dict) or not isinstance(signing, dict):
        raise ValueError("candidate manifest identity is malformed")
    version = release.get("version")
    channel = release.get("channel")
    source_commit = release.get("commit")
    key_id = signing.get("key_id")
    if not all(isinstance(value, str) for value in (version, channel, source_commit, key_id)):
        raise ValueError("candidate manifest release identity is incomplete")
    require_commit(source_commit, "candidate source commit")
    if channel not in {"stable", "preview"}:
        raise ValueError("candidate release channel is invalid")

    workflow_run = candidate_run_identity(
        arguments.candidate_run,
        version=version,
        repository=arguments.repository,
        source_commit=source_commit,
    )

    candidate_version = (candidate / "VERSION").read_text(encoding="utf-8").strip()
    if version != candidate_version or version.lower() == "next":
        raise ValueError("candidate VERSION differs from its signed manifest")
    signed_bundle = file_identity(arguments.signed_bundle)
    expected_bundle_name = f"prns-flasher-candidate-v{version}-signed.tar.gz"
    if signed_bundle["name"] != expected_bundle_name:
        raise ValueError(f"signed candidate must be named {expected_bundle_name}")
    bind_candidate_to_signed_archive(candidate, arguments.signed_bundle)
    qualification_evidence = file_identity(arguments.qualification_evidence)
    expected_evidence_name = f"qualification-evidence-v{version}.tar.gz"
    if qualification_evidence["name"] != expected_evidence_name:
        raise ValueError(f"qualification evidence must be named {expected_evidence_name}")
    if qualification_evidence["size"] == 0:
        raise ValueError("qualification evidence archive is empty")

    acceptance = load_object(arguments.acceptance, "acceptance record")
    acceptance_candidate = acceptance.get("candidate")
    if not isinstance(acceptance_candidate, dict):
        raise ValueError("acceptance record has no candidate identity")
    expected_acceptance_identity = {
        "version": version,
        "channel": channel,
        "source_commit": source_commit,
        "signing_key_id": key_id,
        "manifest_sha256": sha256(manifest_path),
        "manifest_signature_sha256": sha256(Path(f"{manifest_path}.minisig")),
        "signed_candidate_sha256": signed_bundle["sha256"],
        "prerelease_published_at": arguments.prerelease_published_at,
    }
    actual_acceptance_identity = dict(acceptance_candidate)
    actual_key_id = actual_acceptance_identity.get("signing_key_id")
    if isinstance(actual_key_id, str):
        actual_acceptance_identity["signing_key_id"] = actual_key_id.upper()
        expected_acceptance_identity["signing_key_id"] = key_id.upper()
    if actual_acceptance_identity != expected_acceptance_identity:
        raise ValueError("acceptance record does not bind the exact signed candidate")
    require_commit(arguments.acceptance_source_commit, "acceptance evidence source commit")
    acceptance_signature = Path(f"{arguments.acceptance}.minisig")
    acceptance_identity = file_identity(arguments.acceptance)
    if not acceptance_signature.is_file():
        raise ValueError("acceptance record has no Minisign signature")
    acceptance_identity.update(
        {
            "signature_sha256": sha256(acceptance_signature),
            "source_commit": arguments.acceptance_source_commit,
        }
    )

    channel_files = sorted((candidate / "channels").glob("*.json"))
    if len(channel_files) != 1 or channel_files[0].stem != channel:
        raise ValueError("candidate channel descriptor is missing or ambiguous")
    audit_path = candidate / "audit" / "release-audit-evidence.md"
    metadata_path = candidate / "metadata" / "build.json"
    tester_roster_path = candidate / "qualification" / "tester-roster.json"
    if not audit_path.is_file() or not audit_path.read_bytes():
        raise ValueError("candidate audit evidence is unavailable")
    if not metadata_path.is_file():
        raise ValueError("candidate build metadata is unavailable")
    if not tester_roster_path.is_file():
        raise ValueError("candidate tester roster is unavailable")
    hotfix_identity = None
    hotfix_metadata_path = candidate / "metadata" / "hotfix.json"
    if hotfix_metadata_path.is_file():
        hotfix_spec_path = candidate / "qualification" / "hotfix.json"
        if not hotfix_spec_path.is_file():
            raise ValueError("candidate hotfix specification is unavailable")
        hotfix_identity = {
            "inheritance": {
                "path": "metadata/hotfix.json",
                "sha256": sha256(hotfix_metadata_path),
            },
            "specification": {
                "path": "qualification/hotfix.json",
                "sha256": sha256(hotfix_spec_path),
            },
        }

    attestation_bundle = load_object(arguments.attestation_bundle, "attestation bundle")
    actual_subjects = attestation_subjects(attestation_bundle)
    attestation = load_object(arguments.attestation_metadata, "attestation metadata")
    expected_metadata_fields = {
        "schema",
        "repository",
        "workflow_ref",
        "workflow_sha",
        "workflow_run_id",
        "attestation_id",
        "attestation_url",
        "bundle",
        "subjects",
    }
    if set(attestation) != expected_metadata_fields or attestation.get("schema") != 1:
        raise ValueError("attestation metadata has an unsupported shape")
    if attestation.get("repository") != arguments.repository:
        raise ValueError("attestation repository differs from the release repository")
    expected_workflow_prefix = (
        f"{arguments.repository}/.github/workflows/flasher-sign.yml@refs/heads/"
    )
    if not str(attestation.get("workflow_ref", "")).startswith(expected_workflow_prefix):
        raise ValueError("attestation was not produced by the protected flasher signer")
    if attestation.get("workflow_sha") != source_commit:
        raise ValueError("attestation signer implementation differs from the candidate source")
    expected_url_prefix = f"https://github.com/{arguments.repository}/attestations/"
    if not str(attestation.get("attestation_url", "")).startswith(expected_url_prefix):
        raise ValueError("attestation URL is outside the release repository")
    if attestation.get("bundle") != {
        "name": arguments.attestation_bundle.name,
        "sha256": sha256(arguments.attestation_bundle),
    }:
        raise ValueError("attestation metadata does not bind the exact Sigstore bundle")
    if attestation.get("subjects") != actual_subjects:
        raise ValueError("attestation metadata subjects differ from its signed statement")

    required_subjects = [(arguments.signed_bundle.name, arguments.signed_bundle)]
    for target, extension in CLI_TARGETS.items():
        name = f"hopspot-flash-{version}-{target}{extension}"
        required_subjects.append(
            (f"cli/{name}", candidate / "cli" / name)
        )

    firmware = []
    seen_firmware_paths = set()
    targets = manifest.get("targets")
    if not isinstance(targets, list):
        raise ValueError("candidate manifest has no firmware targets")
    for target in targets:
        if not isinstance(target, dict) or not isinstance(target.get("board_slug"), str):
            raise ValueError("candidate manifest contains a malformed firmware target")
        parts = target_artifacts(target)
        for part in parts:
            if not isinstance(part, dict):
                raise ValueError("candidate manifest contains a malformed firmware part")
            relative = part.get("path")
            size = part.get("size")
            checksum = part.get("sha256")
            if (
                not isinstance(relative, str)
                or relative in seen_firmware_paths
                or not isinstance(size, int)
                or isinstance(size, bool)
                or not isinstance(checksum, str)
            ):
                raise ValueError("candidate manifest contains an invalid firmware identity")
            seen_firmware_paths.add(relative)
            artifact = safe_candidate_path(candidate, relative)
            if (
                not artifact.is_file()
                or artifact.stat().st_size != size
                or sha256(artifact) != checksum
            ):
                raise ValueError(f"manifest firmware identity disagrees with {relative}")
            required_subjects.append((relative, artifact))
            firmware.append(
                {
                    "board_slug": target["board_slug"],
                    "path": relative,
                    "size": size,
                    "sha256": checksum,
                }
            )

    expected_subjects = {
        (name, sha256(path))
        for name, path in required_subjects
        if path.is_file()
    }
    if len(expected_subjects) != len(required_subjects):
        raise ValueError("release attestation inputs are missing or have duplicate identities")
    attested_subjects = {
        (subject["name"], subject["sha256"]) for subject in actual_subjects
    }
    archive_coverage_record = None
    if attested_subjects != expected_subjects:
        missing = expected_subjects - attested_subjects
        unexpected = attested_subjects - expected_subjects
        archive_coverage_record = archive_coverage(
            version=version,
            source_commit=source_commit,
            signed_bundle=signed_bundle,
            attestation_bundle_sha256=sha256(arguments.attestation_bundle),
            attestation_workflow_run_id=attestation["workflow_run_id"],
            attested_subjects=attested_subjects,
            missing=missing,
            unexpected=unexpected,
        )
    if attested_subjects != expected_subjects and archive_coverage_record is None:
        raise ValueError(
            f"GitHub attestation subjects differ from release paths; "
            f"missing={sorted(missing)}, unexpected={sorted(unexpected)}"
        )

    public_review = public_review_identity(
        arguments.public_review_evidence,
        repository=arguments.repository,
        version=version,
        source_commit=source_commit,
        signed_bundle_sha256=str(signed_bundle["sha256"]),
        manifest_sha256=sha256(manifest_path),
        prerelease_published_at=arguments.prerelease_published_at,
    )
    if (
        public_review["workflow_path"] == WORKFLOW_PATH
        and public_review["workflow_run_id"] != attestation.get("workflow_run_id")
    ):
        raise ValueError(
            "public-review evidence was not produced by the attested signing run"
        )
    if public_review["workflow_path"] not in {WORKFLOW_PATH, SUITE_WORKFLOW_PATH}:
        raise ValueError("public-review evidence was not produced by a release workflow")

    return {
        "schema": 1,
        "release": {
            "version": version,
            "channel": channel,
            "source_commit": source_commit,
            "signing_key_id": key_id.upper(),
            "prerelease_published_at": arguments.prerelease_published_at,
        },
        "candidate": {
            "archive": signed_bundle,
            "workflow_run": workflow_run,
            "manifest": document_identity(manifest_path),
            "channel_descriptor": {
                "name": channel_files[0].name,
                **document_identity(channel_files[0]),
            },
            "checksums": document_identity(candidate / "SHA256SUMS.txt"),
            "build_metadata": {
                "path": "metadata/build.json",
                "sha256": sha256(metadata_path),
            },
            "audit_evidence": {
                "path": "audit/release-audit-evidence.md",
                "sha256": sha256(audit_path),
            },
            "tester_roster": {
                "path": "qualification/tester-roster.json",
                "sha256": sha256(tester_roster_path),
            },
            **({"hotfix": hotfix_identity} if hotfix_identity is not None else {}),
            "firmware": sorted(
                firmware, key=lambda item: (item["board_slug"], item["path"])
            ),
        },
        "acceptance": acceptance_identity,
        "qualification_evidence": qualification_evidence,
        "public_review": public_review,
        "attestation": {
            "metadata": attestation,
            "metadata_file": file_identity(arguments.attestation_metadata),
            **(
                {"archive_coverage": archive_coverage_record}
                if archive_coverage_record is not None
                else {}
            ),
        },
    }


def add_common_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-run", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--acceptance", type=Path, required=True)
    parser.add_argument("--acceptance-source-commit", required=True)
    parser.add_argument("--qualification-evidence", type=Path, required=True)
    parser.add_argument("--public-review-evidence", type=Path, required=True)
    parser.add_argument("--prerelease-published-at", required=True)
    parser.add_argument("--attestation-bundle", type=Path, required=True)
    parser.add_argument("--attestation-metadata", type=Path, required=True)
    parser.add_argument("--repository", required=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    create = subparsers.add_parser("create")
    add_common_arguments(create)
    create.add_argument("--output", type=Path, required=True)
    verify = subparsers.add_parser("verify")
    add_common_arguments(verify)
    verify.add_argument("--release-record", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        expected = build_record(arguments)
        if arguments.command == "create":
            arguments.output.parent.mkdir(parents=True, exist_ok=True)
            arguments.output.write_text(
                json.dumps(expected, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
                newline="\n",
            )
            print(arguments.output)
        else:
            actual = load_object(arguments.release_record, "release record")
            if actual != expected:
                raise ValueError("release record does not match the exact release evidence")
            print(
                f"verified flasher release record {actual['release']['version']} "
                f"from {actual['release']['source_commit']}"
            )
    except (
        OSError,
        ValueError,
        json.JSONDecodeError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"flasher release record validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
