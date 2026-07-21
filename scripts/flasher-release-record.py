#!/usr/bin/env python3
"""Create or verify the signed record that binds a qualified flasher release."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from flasher_release_evidence import attestation_subjects, sha256


CLI_TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
}
FLASHER_CANDIDATE_WORKFLOW = ".github/workflows/flasher-candidate.yml"


def file_identity(path: Path) -> dict[str, str | int]:
    if not path.is_file():
        raise ValueError(f"release evidence file is unavailable: {path}")
    return {"name": path.name, "size": path.stat().st_size, "sha256": sha256(path)}


def document_identity(document: Path) -> dict[str, str]:
    signature = Path(f"{document}.minisig")
    if not document.is_file() or not signature.is_file():
        raise ValueError(f"signed release document is incomplete: {document}")
    return {"sha256": sha256(document), "signature_sha256": sha256(signature)}


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


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
    candidate = arguments.candidate.resolve()
    manifest_path = candidate / "flash-manifest.json"
    manifest = load_object(manifest_path, "candidate manifest")
    release = manifest.get("release")
    signing = manifest.get("signing")
    if manifest.get("schema") != 2 or not isinstance(release, dict) or not isinstance(signing, dict):
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
    if not audit_path.is_file() or not audit_path.read_bytes():
        raise ValueError("candidate audit evidence is unavailable")
    if not metadata_path.is_file():
        raise ValueError("candidate build metadata is unavailable")

    attestation_bundle = load_object(arguments.attestation_bundle, "attestation bundle")
    actual_subjects = attestation_subjects(attestation_bundle)
    attestation = load_object(arguments.attestation_metadata, "attestation metadata")
    expected_metadata_fields = {
        "schema",
        "repository",
        "workflow_ref",
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

    required_subjects = [arguments.signed_bundle]
    for target, extension in CLI_TARGETS.items():
        required_subjects.append(
            candidate / "cli" / f"hopspot-flash-{version}-{target}{extension}"
        )
    attested_hashes = {subject["sha256"] for subject in actual_subjects}
    for required in required_subjects:
        if not required.is_file() or sha256(required) not in attested_hashes:
            raise ValueError(f"GitHub attestation does not cover {required.name}")

    return {
        "schema": 1,
        "release": {
            "version": version,
            "channel": channel,
            "source_commit": source_commit,
            "signing_key_id": key_id.upper(),
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
        },
        "acceptance": acceptance_identity,
        "attestation": attestation,
    }


def add_common_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-run", type=Path, required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--acceptance", type=Path, required=True)
    parser.add_argument("--acceptance-source-commit", required=True)
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
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"flasher release record validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
