#!/usr/bin/env python3
"""Audit the selected shipping Xtensa ELFs after their owning resource reports.

Known non-16-byte immediate adjustments fail. Dynamic adjustments remain
unresolved in the reports; this is not a whole-program or runtime stack proof.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
TARGET = "xtensa-esp32s3-none-elf"
POLICY = ["-C", "link-arg=-Tlinkall.x", "-C", "force-frame-pointers=no"]


def component(value: str) -> str:
    if not value or value in (".", "..") or Path(value).name != value:
        raise ValueError(f"invalid owned artifact component: {value!r}")
    return value


def validate_counts(facts: dict, target: str) -> None:
    names = ("functions", "instruction_count", "movsp_count", "misaligned_immediate_count", "unknown_adjustment_count")
    if any(type(facts.get(name)) is not int or facts[name] < 0 for name in names):
        raise RuntimeError(f"malformed frame-audit counts: {target}")
    if facts["functions"] == 0:
        raise RuntimeError(f"no function symbols inspected: {target}")
    if facts["instruction_count"] == 0:
        raise RuntimeError(f"no instructions inspected: {target}")
    if facts["movsp_count"] > facts["instruction_count"]:
        raise RuntimeError(f"inconsistent instruction count: {target}")
    if facts["misaligned_immediate_count"] + facts["unknown_adjustment_count"] > facts["movsp_count"]:
        raise RuntimeError(f"inconsistent frame-audit counts: {target}")


def check(root: Path) -> list[dict]:
    baseline = json.loads((root / "personal-hopspot/resources/baseline/canonical.json").read_text())
    targets = [row["target"]["id"] for row in baseline["targets"]
               if row["architecture"]["rust_target"] == TARGET]
    if not targets or len(set(targets)) != len(targets):
        raise RuntimeError("canonical baseline must select distinct shipping Xtensa targets")
    artifacts = root / "target/flash-artifacts/resources/configured"
    results = []
    failures = []
    for target in targets:
        component(target)
        report = json.loads((artifacts / "reports" / (target + ".json")).read_text())
        if (report["target"]["id"] != target or report["status"] != {"kind": "success"}
                or report["architecture"]["rust_target"] != TARGET
                or report["architecture"]["rustflags"] != POLICY):
            raise RuntimeError(f"successful current-policy resource report required: {target}")
        binary = component(report["build"]["binary"])
        elf = artifacts / "cargo-cache" / TARGET / "release" / binary
        output = artifacts / "work" / target / "frame-alignment.json"
        output.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([sys.executable, str(root / "validation/platforms/xtensa-frame-audit.py"),
                        str(elf), str(output), "--objdump", "xtensa-esp-elf-objdump"], check=True)
        facts = json.loads(output.read_text())
        validate_counts(facts, target)
        if facts["sha256"] != hashlib.sha256(elf.read_bytes()).hexdigest():
            raise RuntimeError(f"ELF changed during audit: {target}")
        results.append({"target": target, **{key: facts[key] for key in (
            "sha256", "functions", "instruction_count", "movsp_count", "misaligned_immediate_count",
            "unknown_adjustment_count")}})
        if facts["misaligned_immediate_count"]:
            failures.append(target)
    summary = {"scope": "known immediate MOVSP alignment; dynamic cases unresolved; no device access",
               "profiles": results, "failed_targets": failures}
    (artifacts / "xtensa-frame-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    if failures:
        raise RuntimeError(f"misaligned immediate stack adjustments: {', '.join(failures)}")
    return results


if __name__ == "__main__":
    rows = check(ROOT)
    print(f"XTENSA_KNOWN_IMMEDIATE_FRAME_GATE_OK targets={len(rows)} "
          f"unresolved={sum(row['unknown_adjustment_count'] for row in rows)}")
