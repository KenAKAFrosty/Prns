#!/usr/bin/env python3
"""Fail closed when workflows regain mutable inputs or unbounded CI resources."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = tuple(sorted((ROOT / ".github" / "workflows").glob("*.yml")))
ACTION_PATTERN = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
JOB_PATTERN = re.compile(r"(?m)^  ([A-Za-z0-9_-]+):\n")
AUTOMATIC_WORKFLOWS = frozenset(
    {
        "ci.yml",
        "deep-validation.yml",
        "hardening.yml",
        "host-sdks.yml",
        "mutation-audit.yml",
        "napi.yml",
    }
)
STANDARD_MATRIX_PARALLELISM_LIMIT = 4
MATRIX_PARALLELISM_LIMITS = {"release-readiness.yml": 20}
MAIN_RELEASE_AUTHORITY_WORKFLOWS = (
    "host-sdk-promote.yml",
    "host-sdk-public-qualification.yml",
    "host-sdk-stage.yml",
    "prnsd-candidate.yml",
    "prnsd-image-candidate.yml",
    "prnsd-staging-publish.yml",
    "prnsd-staging-qualification.yml",
    "suite-deployment-qualification.yml",
    "suite-promote.yml",
    "suite-sign.yml",
)
TRUSTED_DISPATCH_CHECKOUTS = {
    "flasher-installation-qualification.yml": ("inputs.source_commit",),
    "host-sdk-public-qualification.yml": ("inputs.expected_sha",),
    "prnsd-candidate.yml": ("inputs.commit_sha",),
    "prnsd-image-candidate.yml": ("inputs.commit_sha",),
    "prnsd-staging-qualification.yml": ("inputs.source_commit",),
    "release-readiness.yml": (
        "inputs.commit_sha",
        "needs.inventory.outputs.commit",
    ),
    "suite-deployment-qualification.yml": ("inputs.source_commit",),
}


def workflow_jobs(text: str) -> tuple[tuple[str, str], ...]:
    jobs = text.find("\njobs:\n")
    if jobs < 0:
        return ()
    body = text[jobs + len("\njobs:\n") :]
    matches = tuple(JOB_PATTERN.finditer(body))
    return tuple(
        (
            match.group(1),
            body[match.end() : matches[index + 1].start()]
            if index + 1 < len(matches)
            else body[match.end() :],
        )
        for index, match in enumerate(matches)
    )


def validate_resource_bounds(workflow: Path, text: str) -> list[str]:
    errors: list[str] = []
    relative = workflow.relative_to(ROOT)
    artifact_limit = 7 if workflow.name in AUTOMATIC_WORKFLOWS else 30
    parallelism_limit = MATRIX_PARALLELISM_LIMITS.get(
        workflow.name, STANDARD_MATRIX_PARALLELISM_LIMIT
    )
    for job_name, block in workflow_jobs(text):
        if re.search(r"(?m)^    runs-on:", block):
            timeout = re.search(r"(?m)^    timeout-minutes:\s*(\d+)\s*$", block)
            if timeout is None:
                errors.append(f"{relative}: {job_name} has no explicit timeout")
            elif not 1 <= int(timeout.group(1)) <= 360:
                errors.append(f"{relative}: {job_name} has an invalid timeout")

        strategy = re.search(
            r"(?ms)^    strategy:\n(.*?)(?=^    [A-Za-z0-9_-]+:|\Z)", block
        )
        if strategy is not None and re.search(r"(?m)^      matrix:", strategy.group(1)):
            parallel = re.search(
                r"(?m)^      max-parallel:\s*(\d+)\s*$", strategy.group(1)
            )
            if parallel is None:
                errors.append(f"{relative}: {job_name} has an unbounded matrix")
            elif not 1 <= int(parallel.group(1)) <= parallelism_limit:
                errors.append(
                    f"{relative}: {job_name} exceeds {parallelism_limit} parallel matrix jobs"
                )
            elif (
                workflow.name in MATRIX_PARALLELISM_LIMITS
                and int(parallel.group(1)) != parallelism_limit
            ):
                errors.append(
                    f"{relative}: {job_name} must use all {parallelism_limit} "
                    "parallel matrix jobs"
                )

        steps = re.split(r"(?m)(?=^      - )", block)
        for step in steps:
            if not re.search(r"(?m)^\s+uses: actions/upload-(?:artifact|pages-artifact)@", step):
                continue
            retention = re.search(r"(?m)^          retention-days:\s*(\d+)\s*$", step)
            if retention is None:
                errors.append(f"{relative}: {job_name} uploads an artifact without retention")
            elif not 1 <= int(retention.group(1)) <= artifact_limit:
                errors.append(
                    f"{relative}: {job_name} artifact retention exceeds {artifact_limit} days"
                )
    return errors


def validate() -> list[str]:
    errors: list[str] = []
    lock_path = ROOT / "release" / "flash" / "action-pins.json"
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    actions = lock.get("actions")
    if lock.get("schema") != 1 or not isinstance(actions, dict):
        return ["release/flash/action-pins.json has an unsupported shape"]

    used: set[str] = set()
    for workflow in WORKFLOWS:
        text = workflow.read_text(encoding="utf-8")
        errors.extend(validate_resource_bounds(workflow, text))
        for reference in ACTION_PATTERN.findall(text):
            if reference.startswith("./"):
                continue
            if "@" not in reference:
                errors.append(f"{workflow.relative_to(ROOT)}: action has no ref: {reference}")
                continue
            action, revision = reference.rsplit("@", maxsplit=1)
            used.add(action)
            pin = actions.get(action)
            if not isinstance(pin, dict):
                errors.append(f"{workflow.relative_to(ROOT)}: {action} is absent from action-pins.json")
                continue
            expected = pin.get("sha")
            if not isinstance(expected, str) or not SHA_PATTERN.fullmatch(expected):
                errors.append(f"action-pins.json has an invalid SHA for {action}")
            elif revision != expected:
                errors.append(
                    f"{workflow.relative_to(ROOT)}: {action}@{revision} must use {expected}"
                )

    unused = sorted(set(actions) - used)
    if unused:
        errors.append(f"action-pins.json contains unused actions: {unused}")

    for workflow_name, untrusted_refs in TRUSTED_DISPATCH_CHECKOUTS.items():
        release_workflow = (
            ROOT / ".github" / "workflows" / workflow_name
        ).read_text(encoding="utf-8")
        if "ref: ${{ github.sha }}" not in release_workflow:
            errors.append(f"{workflow_name} does not checkout the dispatched workflow SHA")
        for untrusted_ref in untrusted_refs:
            fragment = f"ref: ${{{{ {untrusted_ref} }}}}"
            if fragment in release_workflow:
                errors.append(
                    f"{workflow_name} checks out untrusted dispatch ref {untrusted_ref}"
                )

    for workflow_name in MAIN_RELEASE_AUTHORITY_WORKFLOWS:
        release_workflow = (
            ROOT / ".github" / "workflows" / workflow_name
        ).read_text(encoding="utf-8")
        if 'refs/heads/main' not in release_workflow:
            errors.append(f"{workflow_name} does not require protected main authority")
        if "github.ref_protected" not in release_workflow:
            errors.append(f"{workflow_name} does not require branch protection")
        if 'refs/heads/trunk' in release_workflow:
            errors.append(f"{workflow_name} retains conflicting trunk release authority")

    candidate = (ROOT / ".github" / "workflows" / "flasher-candidate.yml").read_text(
        encoding="utf-8"
    )
    required_candidate_fragments = (
        "RUSTUP_TOOLCHAIN: 1.96.0",
        "components: llvm-tools-preview",
        "./tools/prns release toolchain esp verify",
        'node-version: "24.18.0"',
        'version: "1.21.0"',
        "cargo binstall --locked --no-confirm --force dioxus-cli@0.7.5",
        "link-arg=/Brepro",
        "./tools/prns release candidate finalize --",
        "./tools/prns release candidate package --",
        "./tools/prns release candidate compare --",
        "./tools/prns release candidate extract --",
        "./tools/prns release candidate validate-unsigned --",
        "default: retain",
        "./tools/prns release website-history -- guard-bootstrap",
        "finalized flasher release record",
        'record="$assets/flasher-release-record-v${HISTORY_VERSION}.json"',
        "./tools/prns release website-history -- probe-stable",
        "flasher-release-history-${{ github.run_id }}",
        "EXPECTED_HISTORY_SHA256",
        "PRNS_RELEASE_HISTORY",
        "targetCommitish",
        "./tools/prns release historical verify --",
        "--public-review-evidence",
        "actions/runs/${review_run_id}/attempts/${review_attempt}",
        "release/acceptance/rosters/$(cat VERSION).json",
    )
    for fragment in required_candidate_fragments:
        if fragment not in candidate:
            errors.append(f"flasher-candidate.yml is missing exact release pin {fragment!r}")
    for mutable in (
        "ubuntu-latest",
        "windows-latest",
        "@main",
        "@stable",
        'node-version: "20"',
    ):
        if mutable in candidate:
            errors.append(f"flasher-candidate.yml contains mutable production input {mutable!r}")

    daemon_candidate = (
        ROOT / ".github" / "workflows" / "prnsd-candidate.yml"
    ).read_text(encoding="utf-8")
    for linkage_gate in (
        "components: llvm-tools-preview",
        'sysroot="$(cygpath --unix "$(rustc --print sysroot)")"',
        'host="$(rustc --print host-tuple)"',
        'llvm_readobj="${sysroot}/lib/rustlib/${host}/bin/llvm-readobj.exe"',
        'test -x "$llvm_readobj"',
        '"$llvm_readobj" --coff-imports',
    ):
        if linkage_gate not in daemon_candidate:
            errors.append(
                "prnsd-candidate.yml is missing deterministic Windows linkage gate "
                f"{linkage_gate!r}"
            )
    if "rustup which llvm-readobj" in daemon_candidate:
        errors.append(
            "prnsd-candidate.yml must resolve llvm-readobj from the pinned Rust sysroot"
        )

    image_candidate = (
        ROOT / ".github" / "workflows" / "prnsd-image-candidate.yml"
    ).read_text(encoding="utf-8")
    for source_gate in (
        'test "$GITHUB_REF" = "refs/heads/main"',
        'test "$REF_PROTECTED" = "true"',
        "./tools/prns release source package --",
        '--commit "$GITHUB_SHA"',
        "--output target/source-bundle/source.zip",
        "--source-archive-checksum target/source-bundle/source.zip.sha256",
    ):
        if source_gate not in image_candidate:
            errors.append(
                "prnsd-image-candidate.yml is missing exact image source gate "
                f"{source_gate!r}"
            )

    staging_publication = (
        ROOT / ".github" / "workflows" / "prnsd-staging-publish.yml"
    ).read_text(encoding="utf-8")
    for staging_gate in (
        "Require exact protected staging source",
        'test "$GITHUB_REF" = "refs/heads/main"',
        'test "$REF_PROTECTED" = "true"',
        "Verify image candidate workflow custody",
        ".github/workflows/prnsd-image-candidate.yml",
        "head_branch' \"${RUNNER_TEMP}/image-run.json\")\" = \"main\"",
        "image-candidate-verify",
        "ghcr.io/kenakafrosty/prnsd-staging",
        "candidate-${GITHUB_SHA}-${architecture}",
        "--preserve-digests",
        "package_is_public",
        "Verify explicitly public staging visibility",
        "staging-metadata",
        "staging-railway-contract",
    ):
        if staging_gate not in staging_publication:
            errors.append(
                "prnsd-staging-publish.yml is missing staging isolation gate "
                f"{staging_gate!r}"
            )
    for release_mutation in (
        "gh release",
        "ghcr.io/kenakafrosty/prnsd:candidate-",
        "ghcr.io/kenakafrosty/prnsd:latest",
        "contents: write",
        "release-signing",
        "public-release",
    ):
        if release_mutation in staging_publication:
            errors.append(
                "prnsd-staging-publish.yml crosses into release authority with "
                f"{release_mutation!r}"
            )

    staging_qualification = (
        ROOT / ".github" / "workflows" / "prnsd-staging-qualification.yml"
    ).read_text(encoding="utf-8")
    for staging_gate in (
        'test "$GITHUB_REF" = "refs/heads/main"',
        'test "$REF_PROTECTED" = "true"',
        "Verify public staging publication custody",
        ".github/workflows/prnsd-staging-publish.yml",
        "head_branch' \"${RUNNER_TEMP}/publication-run.json\")\" = \"main\"",
        ".inputs.package_is_public",
        "staging-metadata-verify",
        "ghcr.io/kenakafrosty/prnsd-staging",
        ".platform_environment.RAILWAY_RUN_UID",
        "docker pull --platform linux/amd64",
        "docker pull --platform linux/arm64",
        "persistence_restored",
        "rollback_completed",
        "single_replica",
        "staging-deployment-evidence",
    ):
        if staging_gate not in staging_qualification:
            errors.append(
                "prnsd-staging-qualification.yml is missing live staging gate "
                f"{staging_gate!r}"
            )
    for release_mutation in (
        "gh release",
        "ghcr.io/kenakafrosty/prnsd@",
        "contents: write",
        "packages: write",
        "deployment-qualification-v",
        "environment: release-qualification",
    ):
        if release_mutation in staging_qualification:
            errors.append(
                "prnsd-staging-qualification.yml crosses into release authority with "
                f"{release_mutation!r}"
            )

    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    readiness = (
        ROOT / ".github" / "workflows" / "release-readiness.yml"
    ).read_text(encoding="utf-8")
    for preflight_fragment in (
        "astral-sh/setup-uv@d4b2f3b6ecc6e67c4457f6d3e41ec42d3d0fcb86",
        'python3 validation/run.py run --suite release-contracts --expected-sha "$GITHUB_SHA"',
    ):
        if preflight_fragment not in ci:
            errors.append(
                f"ci.yml is missing release critical-path preflight {preflight_fragment!r}"
            )
    release_contracts = (
        ROOT / "validation" / "release" / "contracts.sh"
    ).read_text(encoding="utf-8")
    for preflight_fragment in (
        "uvx --from ruff==0.15.22 ruff check",
        "--select F821",
        "tools/release",
        "tools/tests",
        "validation/release",
        "-m unittest discover",
    ):
        if preflight_fragment not in release_contracts:
            errors.append(
                "release contracts are missing critical-path preflight "
                f"{preflight_fragment!r}"
            )
    locked_dioxus = "cargo binstall --locked --no-confirm --force dioxus-cli@0.7.5"
    for name, workflow in (
        ("ci.yml", ci),
        ("flasher-candidate.yml", candidate),
        ("release-readiness.yml", readiness),
    ):
        if locked_dioxus not in workflow:
            errors.append(f"{name} does not install the exact Dioxus CLI from its lockfile")
    qualify = dict(workflow_jobs(readiness)).get("qualify", "")
    esp_dioxus_condition = "if: matrix.group == 'web' || matrix.group == 'esp'"
    cargo_binstall_step = re.search(
        r"uses: cargo-bins/cargo-binstall@[0-9a-f]{40}\n"
        rf"        {re.escape(esp_dioxus_condition)}\n"
        r'        with:\n          version: "1\.21\.0"',
        qualify,
    )
    dioxus_step = re.search(
        r"name: Prepare exact web and ESP tool\n"
        rf"        {re.escape(esp_dioxus_condition)}\n"
        rf"        run: {re.escape(locked_dioxus)}",
        qualify,
    )
    if cargo_binstall_step is None or dioxus_step is None:
        errors.append(
            "release-readiness.yml does not provision exact cargo-binstall and Dioxus "
            "tools for both web and ESP suites"
        )
    qualify_toolchain = re.search(
        r"(?m)^    env:\n      RUSTUP_TOOLCHAIN: 1\.96\.0$",
        qualify,
    )
    if qualify_toolchain is None:
        errors.append(
            "release-readiness.yml does not force the exact Rust toolchain for "
            "qualification suites"
        )
    embedded_target_step = re.search(
        r"name: Prepare embedded targets\n"
        r"        if: matrix\.group == 'embedded' \|\| matrix\.group == 'esp'\n"
        r"        run: rustup target add --toolchain 1\.96\.0 "
        r"riscv32imac-unknown-none-elf "
        r"thumbv7em-none-eabihf",
        qualify,
    )
    if embedded_target_step is None:
        errors.append(
            "release-readiness.yml does not provision embedded Rust targets for ESP suites"
        )
    for mutation_fragment in (
        "matrix.domain == 'mutation'",
        "cargo-mutants",
        "validation-artifacts/mutation/union/**",
    ):
        if mutation_fragment in readiness:
            errors.append(
                "release-readiness.yml must not place mutation analysis on the release "
                f"critical path: {mutation_fragment!r}"
            )
        if mutation_fragment in ci:
            errors.append(
                "ci.yml must leave mutation analysis to mutation-audit.yml: "
                f"{mutation_fragment!r}"
            )
    for native_package in ("libdbus-1-dev", "pkg-config"):
        if native_package not in readiness:
            errors.append(
                f"release-readiness.yml does not install required package {native_package!r}"
            )
    android_targets = "rustup target add aarch64-linux-android armv7-linux-androideabi"
    if android_targets not in readiness:
        errors.append("release-readiness.yml does not install both Android Rust targets")
    for windows_python_gate in (
        "VALIDATION_PYTHON: ${{ runner.os == 'Windows' && 'python' || 'python3' }}",
        '"$VALIDATION_PYTHON" validation/run.py run',
    ):
        if windows_python_gate not in readiness:
            errors.append(
                "release-readiness.yml is missing Windows validation Python gate "
                f"{windows_python_gate!r}"
            )

    manifest = (ROOT / "validation" / "manifest.toml").read_text(encoding="utf-8")
    if 'nightly = "nightly-2025-11-21"' not in manifest:
        errors.append("validation/manifest.toml does not own the exact nightly pin")
    for workflow_name in (
        "hardening.yml",
        "deep-validation.yml",
        "release-readiness.yml",
    ):
        workflow = (ROOT / ".github" / "workflows" / workflow_name).read_text(
            encoding="utf-8"
        )
        if "toolchain: nightly" in workflow or "toolchain install nightly" in workflow:
            errors.append(f"{workflow_name} installs a floating nightly")
        if "validation/run.py toolchain nightly" not in workflow:
            errors.append(f"{workflow_name} does not resolve the manifest-owned nightly")

    wasm_package = json.loads(
        (ROOT / "prns-wasm" / "package.json").read_text(encoding="utf-8")
    )
    wasm_scripts = wasm_package.get("scripts", {})
    if "stage:docs" in str(wasm_scripts.get("check:events", "")):
        errors.append("prns-wasm check:events must not stage tracked documentation assets")
    if "stage-event-smoke.mjs" not in str(wasm_scripts.get("build:smoke", "")):
        errors.append("prns-wasm build:smoke does not stage its ignored presentation dependency")

    esp_identity = (
        ROOT / "tools" / "release" / "release-esp-toolchain-identity.sh"
    ).read_text(encoding="utf-8")
    esp_installer = (
        ROOT / "tools" / "release" / "install-release-esp-toolchain.sh"
    ).read_text(encoding="utf-8")
    esp_verifier = (
        ROOT / "tools" / "release" / "verify-release-esp-toolchain.sh"
    ).read_text(encoding="utf-8")
    for identity_gate in (
        'ESP_RUSTC_BANNER="rustc 1.95.0-nightly (95e5bda86 2026-04-15) (1.95.0.0)"',
        'ESP_RUSTC_RELEASE="1.95.0-nightly"',
        'ESP_RUSTC_COMMIT_HASH="95e5bda868c960c607597bc03ed9e8f0ad26226d"',
        'ESP_RUSTC_COMMIT_DATE="2026-04-15"',
    ):
        if identity_gate not in esp_identity:
            errors.append(f"ESP toolchain identity is missing exact gate {identity_gate!r}")
    if "verify-release-esp-toolchain.sh" not in esp_installer:
        errors.append("ESP toolchain installer does not reuse the exact identity proof")
    for field in ("banner", "release", "commit_hash", "commit_date"):
        if field not in esp_verifier:
            errors.append(f"ESP toolchain verifier does not check {field}")
    if "RUSTUP_TOOLCHAIN: 1.90.0" not in ci or "toolchain: 1.90.0" not in ci:
        errors.append("ci.yml does not explicitly force and install the Rust 1.90.0 MSRV")
    if 'node-version: "24.18.0"' not in ci:
        errors.append("ci.yml does not test the release web graph with Node 24.18.0")
    for browser_gate in (
        "cargo fmt --manifest-path docs/website/Cargo.toml -- --check",
        "cargo test --manifest-path docs/website/Cargo.toml --locked",
        "playwright install --with-deps chromium",
        "npm run test:browser",
        "npm run test:production-boundary",
    ):
        if browser_gate not in ci:
            errors.append(f"ci.yml is missing required browser gate {browser_gate!r}")
    if "release-critical:" not in ci:
        errors.append("ci.yml lacks the stable release-critical aggregate check")

    deep = (ROOT / ".github" / "workflows" / "deep-validation.yml").read_text(
        encoding="utf-8"
    )
    for resource_gate in (
        'cron: "17 9 1 * *"',
        "group: ${{ github.workflow }}",
        "cancel-in-progress: false",
    ):
        if resource_gate not in deep:
            errors.append(f"deep-validation.yml is missing resource gate {resource_gate!r}")
    for mutation_fragment in (
        "matrix --domain mutation",
        "mutation-aggregate",
        "MUTATION_RESULT",
    ):
        if mutation_fragment in deep:
            errors.append(
                "deep-validation.yml must leave mutation analysis to mutation-audit.yml: "
                f"{mutation_fragment!r}"
            )

    mutation_audit = (
        ROOT / ".github" / "workflows" / "mutation-audit.yml"
    ).read_text(encoding="utf-8")
    for resource_gate in (
        "workflow_dispatch:",
        'cron: "47 10 2 * *"',
        "group: mutation-audit",
        "cancel-in-progress: false",
    ):
        if resource_gate not in mutation_audit:
            errors.append(
                f"mutation-audit.yml is missing resource gate {resource_gate!r}"
            )
    for forbidden_trigger in ("\n  pull_request:", "\n  push:"):
        if forbidden_trigger in mutation_audit:
            errors.append(
                "mutation-audit.yml must remain manual or scheduled: "
                f"{forbidden_trigger.strip()!r}"
            )
    for mutation_gate in (
        "mutation: ${{ steps.matrix.outputs.mutation }}",
        "mutation=$(python3 validation/run.py matrix --domain mutation --tier scheduled)",
        "matrix: ${{ fromJSON(needs.inventory.outputs.mutation) }}",
        "name: mutation-audit-${{ matrix.id }}-${{ github.run_id }}",
        "validation-artifacts/mutation/${{ matrix.id }}/**",
        "pattern: mutation-audit-mutation-analysis-shard-*-${{ github.run_id }}",
        'aggregate --domain mutation --tier scheduled --expected-sha "$GITHUB_SHA"',
    ):
        if mutation_gate not in mutation_audit:
            errors.append(
                f"mutation-audit.yml is missing mutation evidence gate {mutation_gate!r}"
            )

    hardening = (ROOT / ".github" / "workflows" / "hardening.yml").read_text(
        encoding="utf-8"
    )
    for resource_gate in (
        'cron: "41 8 * * 1"',
        "group: ${{ github.workflow }}",
        "cancel-in-progress: false",
    ):
        if resource_gate not in hardening:
            errors.append(f"hardening.yml is missing resource gate {resource_gate!r}")

    host_sdk_promotion = (
        ROOT / ".github" / "workflows" / "host-sdk-promote.yml"
    ).read_text(encoding="utf-8")
    for custody_gate in (
        "group: host-sdk-publication",
        "cancel-in-progress: false",
        "environment: host-sdk-release",
        "HOST_SDK_IMMUTABLE_RELEASES",
        "HOST_SDK_SSH_SIGNING_KEY",
        "Require exact protected stage authority",
        'test "$GITHUB_WORKFLOW_SHA" = "$GITHUB_SHA"',
        "compare/${EXPECTED_SHA}...${GITHUB_SHA}",
        ".github/workflows/host-sdk-stage.yml",
        'test "${run[1]}" = "main"',
        "host-sdk-stage-${{ inputs.expected_sha }}",
        "release.host-sdk.stage.verify",
        "release.host-sdk.promotion.prepare",
        "ssh-keygen -Y sign",
        "git tag -s",
        "git push --atomic",
        "release already exists",
        "gh release create",
        "--verify-tag",
        "--target \"$EXPECTED_SHA\"",
    ):
        if custody_gate not in host_sdk_promotion:
            errors.append(
                f"host-sdk-promote.yml is missing custody gate {custody_gate!r}"
            )
    for forbidden_trigger in ("\n  pull_request:", "\n  push:"):
        if forbidden_trigger in host_sdk_promotion:
            errors.append(
                "host-sdk-promote.yml must remain manual: "
                f"{forbidden_trigger.strip()!r}"
            )
    for release_notes_gate in (
        'contract_abi="$(jq -r .contractAbi dist/promotion/release-index.json)"',
        'schema_version="$(jq -r .schemaVersion dist/promotion/release-index.json)"',
        "Signed ABI $contract_abi, schema $schema_version host SDK artifacts",
    ):
        if release_notes_gate not in host_sdk_promotion:
            errors.append(
                "host-sdk-promote.yml is missing staged contract release notes gate "
                f"{release_notes_gate!r}"
            )
    if "schema 2 host SDK artifacts" in host_sdk_promotion:
        errors.append("host-sdk-promote.yml hardcodes a stale host schema version")

    host_sdk_stage = (
        ROOT / ".github" / "workflows" / "host-sdk-stage.yml"
    ).read_text(encoding="utf-8")
    for reusable_call in (
        """  host:
    needs: custody
    permissions:
      contents: read
      id-token: write
    uses: ./.github/workflows/host-sdks.yml
    with:
      expected_sha: ${{ inputs.expected_sha }}""",
        """  javascript:
    needs: custody
    permissions:
      contents: read
      id-token: write
    uses: ./.github/workflows/napi.yml
    with:
      expected_sha: ${{ inputs.expected_sha }}""",
    ):
        if reusable_call not in host_sdk_stage:
            errors.append(
                "host-sdk-stage.yml does not delegate exact-SHA OIDC authority "
                "to both reusable SDK workflows"
            )

    host_sdks = (
        ROOT / ".github" / "workflows" / "host-sdks.yml"
    ).read_text(encoding="utf-8")
    maven_repository_path = "rs/reticulum/personal-rns/$version"
    if host_sdks.count(maven_repository_path) != 2:
        errors.append(
            "host-sdks.yml does not use the declared Maven group for staging and bundling"
        )
    if "io/reticulum/personal-rns" in host_sdks:
        errors.append("host-sdks.yml retains the obsolete Maven repository path")
    for target, zig_target, wheel in (
        (
            "x86_64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu.2.34",
            "manylinux_2_34_x86_64",
        ),
        (
            "aarch64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu.2.34",
            "manylinux_2_34_aarch64",
        ),
    ):
        matrix_entry = re.compile(
            rf"target: {re.escape(target)}\n"
            rf"\s+zigTarget: {re.escape(zig_target)}\n"
            rf"(?:(?!\n\s+- host:).)*\n\s+audit: {re.escape(wheel)}\n",
            re.DOTALL,
        )
        if matrix_entry.search(host_sdks) is None:
            errors.append(
                "host-sdks.yml does not bind the GNU target, glibc floor, and "
                f"repaired wheel tag for {target}"
            )
    for gnu_gate in (
        "version: 0.14.1",
        "tool: cargo-zigbuild@0.23.0",
        "--target ${{ matrix.settings.zigTarget }}",
        "auditwheel repair",
        "release.host-sdk.python.smoke --",
    ):
        if gnu_gate not in host_sdks:
            errors.append(f"host-sdks.yml is missing GNU capsule gate {gnu_gate!r}")
    if host_sdks.index("auditwheel repair") > host_sdks.index(
        "release.host-sdk.python.smoke --"
    ):
        errors.append("host-sdks.yml smokes repaired GNU wheels before repair")
    for apple_gate in (
        "normalize Apple native binaries",
        'install_name_tool -id "@rpath/${{ matrix.settings.library }}"',
        'strip -S "$release/${{ matrix.settings.library }}"',
        'strip -S "$release/${{ matrix.settings.static }}"',
        '! grep -a -F -q "$HOME"',
    ):
        if apple_gate not in host_sdks:
            errors.append(f"host-sdks.yml is missing Apple capsule gate {apple_gate!r}")
    for workflow_name in (
        "prnsd-candidate.yml",
        "prnsd-image-candidate.yml",
        "prnsd-staging-publish.yml",
        "prnsd-staging-qualification.yml",
        "suite-sign.yml",
        "suite-promote.yml",
    ):
        current_workflow = (
            ROOT / ".github" / "workflows" / workflow_name
        ).read_text(encoding="utf-8")
        if "0.3.1" in current_workflow:
            errors.append(f"{workflow_name} hardcodes the previous product version")

    host_sdk_public = (
        ROOT / ".github" / "workflows" / "host-sdk-public-qualification.yml"
    ).read_text(encoding="utf-8")
    for qualification_gate in (
        "Verify every public signature, hash, tag, and source commit",
        'test "$GITHUB_WORKFLOW_SHA" = "$GITHUB_SHA"',
        'test "$EXPECTED_SHA" = "$GITHUB_SHA"',
        "sha256sum --check SHA256SUMS",
        "ssh-keygen -Y verify",
        "git tag -v",
        "Install npm packages and repeat the persistent two-node journey",
        'cp VERSION "$scratch/VERSION"',
        "release.host-sdk.python.smoke",
        "release.host-sdk.dotnet.smoke -- --public",
        '"personal-rns@=$VERSION"',
        '"hopspot@=$VERSION"',
        '"personal-rns@$VERSION" "hopspot@$VERSION"',
        "https://repo1.maven.org/maven2",
        "persistent-two-node-smoke.c",
        'go -C "$go_root" test ./...',
        "prns-host/bindings/go@v$VERSION",
        'swift test --package-path "$swift_root"',
        '--branch \"v$VERSION\"',
        'Pkg.add(PackageSpec(name=\"PersonalRns\"',
    ):
        if qualification_gate not in host_sdk_public:
            errors.append(
                "host-sdk-public-qualification.yml is missing public gate "
                f"{qualification_gate!r}"
            )
    for forbidden_trigger in ("\n  pull_request:", "\n  push:"):
        if forbidden_trigger in host_sdk_public:
            errors.append(
                "host-sdk-public-qualification.yml must remain manual: "
                f"{forbidden_trigger.strip()!r}"
            )

    napi_release = (ROOT / ".github" / "workflows" / "napi.yml").read_text(
        encoding="utf-8"
    )
    for package_gate in (
        "smoke packed Node and Bun consumers",
        "persistent-two-node-v1.json",
        'cp "$GITHUB_WORKSPACE/VERSION" VERSION',
        "node --test prns-js/tests/native-consumer.test.mjs",
        "npm pack ./personal-hopspot/sdk/hopspot --pack-destination dist/npm",
        '"$GITHUB_WORKSPACE/dist/npm/hopspot-$(cat "$GITHUB_WORKSPACE/VERSION").tgz"',
        "publish alternate-name facade",
        "wait for the canonical package dependency",
        'npm view "personal-rns@$version" version',
        "working-directory: personal-hopspot/sdk/hopspot",
    ):
        if package_gate not in napi_release:
            errors.append(f"napi.yml is missing package journey gate {package_gate!r}")

    signing = (ROOT / ".github" / "workflows" / "flasher-sign.yml").read_text(
        encoding="utf-8"
    )
    for custody_gate in (
        "subject-checksums: target/release/attestation-subjects.sha256",
        'test "$GITHUB_WORKFLOW_SHA" = "$source_commit"',
        "--workflow-sha \"$GITHUB_WORKFLOW_SHA\"",
        "name: Approve protected public release",
        "environment: public-release",
        "./tools/prns release public-review -- create",
        "Publish immutable attempt-specific public-review evidence",
        "public-review-v${RELEASE_VERSION}-run-${GITHUB_RUN_ID}-attempt-${GITHUB_RUN_ATTEMPT}.json",
        "actions/runs/${GITHUB_RUN_ID}/attempts/${GITHUB_RUN_ATTEMPT}",
        "qualification/tester-roster.json",
        "Signed candidate SHA-256:",
        "Manifest SHA-256:",
        "git/matching-refs/tags/${tag}",
        "[.[] | select(.ref == $ref)] | length",
        "--json isDraft,isPrerelease,targetCommitish,name",
        "tag $tag exists without the resumable draft release",
    ):
        if custody_gate not in signing:
            errors.append(f"flasher-sign.yml is missing custody gate {custody_gate!r}")
    if "subject-path:" in signing:
        errors.append("flasher-sign.yml must preserve canonical names with subject-checksums")
    if "targetCommitish,title" in signing:
        errors.append("flasher-sign.yml must use gh release view's supported name field")

    finalization = (
        ROOT / ".github" / "workflows" / "flasher-finalize-evidence.yml"
    ).read_text(encoding="utf-8")
    for finalization_gate in (
        "qualification_evidence_sha256:",
        "qualification-evidence-v${RELEASE_VERSION}.tar.gz",
        "target/candidate/qualification/tester-roster.json",
        "--evidence-root target/qualification-evidence",
        "--prerelease-published-at",
        "--qualification-evidence",
        "--public-review-evidence",
        "Select and verify the exact successful protected public review",
        "actions/runs/${review_run_id}/attempts/${run_attempt}",
        "flasher-release-record-v${RELEASE_VERSION}.json",
    ):
        if finalization_gate not in finalization:
            errors.append(
                f"flasher-finalize-evidence.yml is missing gate {finalization_gate!r}"
            )

    suite_signing = (
        ROOT / ".github" / "workflows" / "suite-sign.yml"
    ).read_text(encoding="utf-8")
    for suite_gate in (
        "Require exact protected release authority",
        "Verify all three producer workflow runs",
        "Verify flasher signing was deferred to suite custody",
        "Require stable flasher release input",
        "suite signing requires a stable flasher candidate; got ${channel}",
        "target/flasher/candidate/channels/stable.json.minisig",
        "required suite asset is missing: ${source}",
        "duplicate suite asset basename: ${source} conflicts with ${destination}",
        "prnsd distribution -- flasher-payloads",
        "git/matching-refs/tags/${tag}",
        "[.[] | select(.ref == $ref)] | length",
        "--json isDraft,isPrerelease,targetCommitish,name",
        "tag $tag exists without the resumable suite prerelease",
        "/attempts/${run_attempt}/jobs?per_page=100",
        "Publish immutable signed candidate as a prerelease",
        "candidate-${GITHUB_SHA}",
        "inventory create",
        "suite-record",
        "name: Approve protected public release",
        "environment: public-release",
        "./tools/prns release public-review -- create",
        "public-review-v${RELEASE_VERSION}-run-${GITHUB_RUN_ID}-attempt-${GITHUB_RUN_ATTEMPT}.json",
    ):
        if suite_gate not in suite_signing:
            errors.append(f"suite-sign.yml is missing custody gate {suite_gate!r}")
    if ".inputs.suite_input" in suite_signing:
        errors.append(
            "suite-sign.yml relies on workflow inputs absent from GitHub's run API"
        )
    if "targetCommitish,title" in suite_signing:
        errors.append("suite-sign.yml must use gh release view's supported name field")
    stable_gate = suite_signing.find("- name: Require stable flasher release input")
    registry_login = suite_signing.find("docker/login-action@")
    if stable_gate < 0 or registry_login < 0 or stable_gate > registry_login:
        errors.append(
            "suite-sign.yml must reject non-stable flasher input before registry login"
        )

    suite_promotion = (
        ROOT / ".github" / "workflows" / "suite-promote.yml"
    ).read_text(encoding="utf-8")
    for suite_gate in (
        "flasher_acceptance_commit:",
        "flasher_finalization_run_id:",
        "flasher_release_record_sha256:",
        'test "$GITHUB_WORKFLOW_SHA" = "$GITHUB_SHA"',
        "compare/${SOURCE_COMMIT}...${{ inputs.flasher_acceptance_commit }}",
        "compare/${{ inputs.flasher_acceptance_commit }}...${GITHUB_SHA}",
        ".github/workflows/flasher-finalize-evidence.yml",
        "flasher-release-record-v${version}.json",
        "./tools/prns release verify --",
        "./tools/prns release public-review -- verify",
        "gh attestation verify \"$bundle\"",
        "docker pull --platform linux/amd64",
        "docker pull --platform linux/arm64",
        "Promote semver and latest only to the verified digest",
    ):
        if suite_gate not in suite_promotion:
            errors.append(f"suite-promote.yml is missing custody gate {suite_gate!r}")

    installation = (
        ROOT / ".github" / "workflows" / "flasher-installation-qualification.yml"
    ).read_text(encoding="utf-8")
    for installation_gate in (
        "Require exact protected default-branch source",
        "Verify signatures, attestations, archives, and signed roster",
        "macos-15",
        "macos-15-intel",
        "ubuntu-24.04",
        "ubuntu-24.04-arm",
        "windows-2025",
        "test \"$GITHUB_SHA\" = \"$SOURCE_COMMIT\"",
        "gh attestation verify",
        "./tools/prns release tester-roster validate --",
        'for installer in install.sh install.ps1; do',
        "./tools/prns release installation-evidence write --",
        'test "$version_output" = "hopspot-flash $RELEASE_VERSION"',
        "installed CLI reported a different version",
        "runner architecture differs from the target archive",
        "flasher-installation-${{ matrix.target }}-${{ github.run_id }}-${{ github.run_attempt }}",
    ):
        if installation_gate not in installation:
            errors.append(
                "flasher-installation-qualification.yml is missing gate "
                f"{installation_gate!r}"
            )
    if "doctor" in installation:
        errors.append(
            "flasher-installation-qualification.yml must not substitute hosted "
            "doctor checks for physical CLI qualification"
        )

    promotion = (ROOT / ".github" / "workflows" / "flasher-promote.yml").read_text(
        encoding="utf-8"
    )
    site = (ROOT / ".github" / "workflows" / "site.yml").read_text(encoding="utf-8")
    for path, workflow in (("flasher-promote.yml", promotion), ("site.yml", site)):
        if "group: prns-public-pages" not in workflow:
            errors.append(f"{path} does not share the serialized Pages custody group")
    for promotion_gate in (
        "--allow-promoted",
        "./tools/prns release assets verify --",
        "permissions:\n      contents: read",
        "rollback_baseline_version",
        "rollback_baseline_kind",
        "rollback_baseline_release_record_sha256",
        "rollback_dry_run_id",
        "rollback_dry_run_attempt",
        "Block promotion without a matching <=15-minute rollback dry-run",
        "./tools/prns release rollback -- validate-record",
        "./tools/prns release rollback -- stage-coming-soon",
        "./tools/prns release rollback -- promotion-state",
        "--baseline-kind",
        "--required-observed-live-state target_baseline",
        "Recheck live promotion CAS immediately before Pages deployment",
        "targetCommitish",
        "./tools/prns release historical verify --",
        'record="target/release/flasher-release-record-v${RELEASE_VERSION}.json"',
        "./tools/prns release public-review -- verify",
        ".public_review.evidence.name",
        "actions/runs/${signing_run_id}/attempts/${run_attempt}",
        "Verify the complete prerelease asset inventory before deployment",
        "--remote-inventory target/prerelease-assets-before-promotion.json",
        "Recheck the verified release asset inventory before deployment",
        "release_asset_inventory_sha256",
        "rollback_baseline_asset_inventory_sha256",
        "[.assets[] | {name, size, digest}] | sort_by(.name)",
        "Verify deployed signed channel and website before release mutation",
        "Bind Pages artifacts to this exact verification attempt",
        "candidate_pages_artifact_name",
        "rollback_baseline_pages_artifact_name",
        "rollback_baseline_stage_artifact_name",
        "restore-baseline-on-failure:",
        "${{ always() && needs.verify.result == 'success'",
        "needs: [verify, publish-and-deploy, post-promotion-smoke, mark-promoted]",
        "Compare-and-swap only the failed candidate back to its verified baseline",
        "target/recovery-live-cas/stable.json",
        "./tools/prns release rollback -- verify-live-website",
        "EXPECTED_CURRENT_ASSET_INVENTORY_SHA256",
        "EXPECTED_BASELINE_ASSET_INVENTORY_SHA256",
        "artifact_name: ${{ needs.verify.outputs.rollback_baseline_pages_artifact_name }}",
        "--prerelease=true --latest=false",
        'gh release edit "v${ROLLBACK_BASELINE_VERSION}" --latest=true',
    ):
        if promotion_gate not in promotion:
            errors.append(f"flasher-promote.yml is missing gate {promotion_gate!r}")
    if "environment: public-release" in promotion:
        errors.append("flasher-promote.yml must not start a second protected approval")
    asset_gate = "Verify the complete prerelease asset inventory before deployment"
    deploy_gate = "actions/deploy-pages@"
    live_verification_gate = (
        "Verify deployed signed channel and website before release mutation"
    )
    release_mutation_gate = (
        "Mark the verified prerelease stable without replacing assets"
    )
    if all(
        gate in promotion
        for gate in (deploy_gate, live_verification_gate, release_mutation_gate)
    ) and not (
        promotion.index(deploy_gate)
        < promotion.index(live_verification_gate)
        < promotion.index(release_mutation_gate)
    ):
        errors.append(
            "flasher-promote.yml must deploy and verify the signed site before "
            "marking the GitHub Release stable"
        )
    if asset_gate in promotion and deploy_gate in promotion and not (
        promotion.index(asset_gate) < promotion.index(deploy_gate)
    ):
        errors.append(
            "flasher-promote.yml must verify every release asset before Pages deployment"
        )
    recovery_job = "restore-baseline-on-failure:"
    recovery_cas = "Compare-and-swap only the failed candidate back to its verified baseline"
    recovery_deploy = "Redeploy the exact verified baseline Pages artifact"
    recovery_verify = "Verify every restored website byte before release rollback"
    recovery_metadata = "Restore baseline release metadata without modifying assets"
    baseline_latest = 'gh release edit "v${ROLLBACK_BASELINE_VERSION}" --latest=true'
    candidate_demote = (
        'gh release edit "v${RELEASE_VERSION}" --prerelease=true --latest=false'
    )
    if recovery_job in promotion:
        recovery = promotion[promotion.index(recovery_job) :]
        ordered_recovery_gates = (
            recovery_cas,
            recovery_deploy,
            recovery_verify,
            recovery_metadata,
            baseline_latest,
            candidate_demote,
        )
        if all(gate in recovery for gate in ordered_recovery_gates) and not all(
            recovery.index(first) < recovery.index(second)
            for first, second in zip(
                ordered_recovery_gates, ordered_recovery_gates[1:]
            )
        ):
            errors.append(
                "flasher-promote.yml must CAS, deploy, verify, restore baseline latest, "
                "and only then demote the failed candidate"
            )

    for site_gate in (
        "Refuse to overwrite any live signed stable channel",
        "./tools/prns release website-history -- probe-stable",
        "steps.custody.outputs.deploy == 'true'",
        "cmp target/live-site-custody/stable.json",
    ):
        if site_gate not in site:
            errors.append(f"site.yml is missing permanent custody gate {site_gate!r}")

    rollback = (ROOT / ".github" / "workflows" / "flasher-rollback.yml").read_text(
        encoding="utf-8"
    )
    for rollback_gate in (
        "group: prns-public-pages",
        "environment: release-rollback",
        "timeout-minutes: 15",
        "./tools/prns release rollback -- live-state",
        "target_kind:",
        "./tools/prns release rollback -- stage-coming-soon",
        "./tools/prns release rollback -- cas-coming-soon",
        "--prerelease=true --latest=false",
        '--mode "$ROLLBACK_MODE"',
        "--mode deploy",
        "./tools/prns release rollback -- validate-record",
        "run-id: ${{ inputs.dry_run_id }}",
        "dry_run_attempt:",
        "--expected-run-attempt",
        "actions/runs/${DRY_RUN_ID}/attempts/${DRY_RUN_ATTEMPT}",
        "actions/upload-pages-artifact@",
        "actions/deploy-pages@",
        "./tools/prns release rollback -- verify-live-website",
        "target/rollback-stage/rollback-stage.json",
        "cmp target/assets-before-latest.json target/assets-after-latest.json",
        "targetCommitish",
        "./tools/prns release historical verify --",
        'record="$assets/flasher-release-record-v${TARGET_VERSION}.json"',
    ):
        if rollback_gate not in rollback:
            errors.append(f"flasher-rollback.yml is missing gate {rollback_gate!r}")
    for forbidden in ("PRNS_MINISIGN_SECRET_KEY_B64", "secrets."):
        if forbidden in rollback:
            errors.append(f"flasher-rollback.yml must not reference {forbidden!r}")
    return errors


def main() -> int:
    try:
        errors = validate()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        errors = [f"workflow pin validation could not run: {error}"]
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print("workflows use reviewed immutable inputs and bounded CI resources")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
