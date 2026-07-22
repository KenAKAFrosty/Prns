#!/usr/bin/env python3
"""Stage rollback websites and create or validate dry-run custody records."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys

from flasher_rollback import (
    create_dry_run_record,
    stage,
    validate_descriptor,
    validate_dry_run_record,
    validate_live_state,
    verify_live_website,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    cas = commands.add_parser("cas")
    cas.add_argument("--descriptor", type=Path, required=True)
    cas.add_argument("--version", required=True)
    cas.add_argument("--manifest-sha256", required=True)
    live_state = commands.add_parser("live-state")
    live_state.add_argument("--descriptor", type=Path, required=True)
    live_state.add_argument("--mode", choices=("dry-run", "deploy"), required=True)
    live_state.add_argument("--target-version", required=True)
    live_state.add_argument("--target-manifest-sha256", required=True)
    live_state.add_argument("--expected-live-version", required=True)
    live_state.add_argument("--expected-live-manifest-sha256", required=True)
    stage_parser = commands.add_parser("stage")
    stage_parser.add_argument("--candidate", type=Path, required=True)
    stage_parser.add_argument("--release-record", type=Path, required=True)
    stage_parser.add_argument("--release-record-sha256", required=True)
    stage_parser.add_argument("--version", required=True)
    stage_parser.add_argument("--output", type=Path, required=True)
    stage_parser.add_argument("--identity-output", type=Path, required=True)
    verify_live = commands.add_parser("verify-live-website")
    verify_live.add_argument("--stage-identity", type=Path, required=True)
    verify_live.add_argument("--site-url", required=True)
    record = commands.add_parser("record")
    record.add_argument("--stage-identity", type=Path, required=True)
    record.add_argument("--expected-live-version", required=True)
    record.add_argument("--expected-live-manifest-sha256", required=True)
    record.add_argument("--repository", required=True)
    record.add_argument("--workflow-run-id", type=int, required=True)
    record.add_argument("--workflow-run-attempt", type=int, required=True)
    record.add_argument("--workflow-job-id", type=int, required=True)
    record.add_argument("--workflow-sha", required=True)
    record.add_argument(
        "--observed-live-state",
        choices=("target_baseline", "expected_live"),
        required=True,
    )
    record.add_argument("--started-epoch", type=int, required=True)
    record.add_argument("--output", type=Path, required=True)
    validate = commands.add_parser("validate-record")
    validate.add_argument("--record", type=Path, required=True)
    validate.add_argument("--run-json", type=Path, required=True)
    validate.add_argument("--job-json", type=Path, required=True)
    validate.add_argument("--stage-identity", type=Path, required=True)
    validate.add_argument("--repository", required=True)
    validate.add_argument("--default-branch", required=True)
    validate.add_argument("--expected-run-id", type=int, required=True)
    validate.add_argument("--expected-run-attempt", type=int, required=True)
    validate.add_argument("--target-version", required=True)
    validate.add_argument("--target-release-record-sha256", required=True)
    validate.add_argument("--expected-live-version", required=True)
    validate.add_argument("--expected-live-manifest-sha256", required=True)
    validate.add_argument("--required-workflow-sha", required=True)
    validate.add_argument(
        "--required-observed-live-state",
        choices=("target_baseline", "expected_live"),
    )
    arguments = parser.parse_args()
    try:
        if arguments.command == "cas":
            validate_descriptor(
                arguments.descriptor, arguments.version, arguments.manifest_sha256
            )
        elif arguments.command == "live-state":
            state = validate_live_state(
                arguments.descriptor,
                mode=arguments.mode,
                target_version=arguments.target_version,
                target_manifest_sha256=arguments.target_manifest_sha256,
                expected_live_version=arguments.expected_live_version,
                expected_live_manifest_sha256=(
                    arguments.expected_live_manifest_sha256
                ),
            )
            print(state)
        elif arguments.command == "stage":
            stage(
                candidate=arguments.candidate,
                release_record=arguments.release_record,
                release_record_sha256=arguments.release_record_sha256,
                version=arguments.version,
                output=arguments.output,
                identity_output=arguments.identity_output,
            )
        elif arguments.command == "verify-live-website":
            verify_live_website(
                stage_identity=arguments.stage_identity,
                site_url=arguments.site_url,
            )
        elif arguments.command == "record":
            create_dry_run_record(
                stage_identity=arguments.stage_identity,
                expected_live_version=arguments.expected_live_version,
                expected_live_manifest_sha256=arguments.expected_live_manifest_sha256,
                repository=arguments.repository,
                workflow_run_id=arguments.workflow_run_id,
                workflow_run_attempt=arguments.workflow_run_attempt,
                workflow_job_id=arguments.workflow_job_id,
                workflow_sha=arguments.workflow_sha,
                observed_live_state=arguments.observed_live_state,
                started_epoch=arguments.started_epoch,
                output=arguments.output,
            )
        else:
            validate_dry_run_record(
                record_path=arguments.record,
                run_json=arguments.run_json,
                job_json=arguments.job_json,
                stage_identity=arguments.stage_identity,
                repository=arguments.repository,
                default_branch=arguments.default_branch,
                expected_run_id=arguments.expected_run_id,
                expected_run_attempt=arguments.expected_run_attempt,
                target_version=arguments.target_version,
                target_release_record_sha256=arguments.target_release_record_sha256,
                expected_live_version=arguments.expected_live_version,
                expected_live_manifest_sha256=arguments.expected_live_manifest_sha256,
                required_workflow_sha=arguments.required_workflow_sha,
                required_observed_live_state=arguments.required_observed_live_state,
            )
    except (OSError, ValueError) as error:
        print(f"flasher rollback custody failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
