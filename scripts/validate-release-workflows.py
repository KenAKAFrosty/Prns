#!/usr/bin/env python3
"""Fail closed when release workflows regain mutable actions or toolchains."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = tuple(sorted((ROOT / ".github" / "workflows").glob("*.yml")))
ACTION_PATTERN = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")


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

    candidate = (ROOT / ".github" / "workflows" / "flasher-candidate.yml").read_text(
        encoding="utf-8"
    )
    required_candidate_fragments = (
        "RUSTUP_TOOLCHAIN: 1.96.0",
        "components: llvm-tools-preview",
        "esp-15.2.0_20250920",
        'node-version: "24.18.0"',
        'version: "1.21.0"',
        "dioxus-cli@0.7.5",
        "link-arg=/Brepro",
        "scripts/compare-flasher-candidates.py",
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

    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    if "RUSTUP_TOOLCHAIN: 1.90.0" not in ci or "toolchain: 1.90.0" not in ci:
        errors.append("ci.yml does not explicitly force and install the Rust 1.90.0 MSRV")
    if 'node-version: "24.18.0"' not in ci:
        errors.append("ci.yml does not test the release web graph with Node 24.18.0")
    for browser_gate in (
        "playwright install --with-deps chromium",
        "npm run test:browser",
        "npm run test:production-boundary",
    ):
        if browser_gate not in ci:
            errors.append(f"ci.yml is missing required browser gate {browser_gate!r}")
    if "release-critical:" not in ci:
        errors.append("ci.yml lacks the stable release-critical aggregate check")

    signing = (ROOT / ".github" / "workflows" / "flasher-sign.yml").read_text(
        encoding="utf-8"
    )
    for custody_gate in (
        "subject-checksums: target/release/attestation-subjects.sha256",
        'test "$GITHUB_WORKFLOW_SHA" = "$source_commit"',
        "--workflow-sha \"$GITHUB_WORKFLOW_SHA\"",
    ):
        if custody_gate not in signing:
            errors.append(f"flasher-sign.yml is missing custody gate {custody_gate!r}")
    if "subject-path:" in signing:
        errors.append("flasher-sign.yml must preserve canonical names with subject-checksums")

    promotion = (ROOT / ".github" / "workflows" / "flasher-promote.yml").read_text(
        encoding="utf-8"
    )
    site = (ROOT / ".github" / "workflows" / "site.yml").read_text(encoding="utf-8")
    for path, workflow in (("flasher-promote.yml", promotion), ("site.yml", site)):
        if "group: prns-public-pages" not in workflow:
            errors.append(f"{path} does not share the serialized Pages custody group")
    for promotion_gate in (
        "--allow-promoted",
        "scripts/verify-flasher-release-assets.py",
        "permissions:\n      contents: read",
    ):
        if promotion_gate not in promotion:
            errors.append(f"flasher-promote.yml is missing gate {promotion_gate!r}")
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
    print("release workflows use only reviewed full-SHA actions and exact production tools")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
