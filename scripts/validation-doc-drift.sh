#!/usr/bin/env bash
# Cheap guard for validation-lane drift: the docs should name the same fuzz
# targets, Kani harnesses, mutation paths, and active RNS reference the repo
# actually runs.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

python3 "${repo_root}/scripts/oracles.py" --list >/dev/null

python3 - "${repo_root}" <<'PY'
from pathlib import Path
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

root = Path(sys.argv[1])
errors = []


def note_error(message):
    errors.append(message)


docs_path = root / "docs/validation.md"
ci_path = root / ".github/workflows/ci.yml"
requirements_path = root / "benchmarks/reference/requirements.txt"
rpc_requirements_path = root / "benchmarks/reference/rpc-requirements.txt"
fuzz_toml_path = root / "fuzz/Cargo.toml"
mutants_toml_path = root / ".cargo/mutants.toml"

docs = docs_path.read_text()
ci = ci_path.read_text()
requirements = requirements_path.read_text().strip()
rpc_requirements = rpc_requirements_path.read_text().strip()

if requirements != "rns==1.4.0":
    note_error(f"{requirements_path.relative_to(root)} pins {requirements!r}, expected 'rns==1.4.0'")
if rpc_requirements != "rns==1.4.0":
    note_error(f"{rpc_requirements_path.relative_to(root)} pins {rpc_requirements!r}, expected 'rns==1.4.0'")

reference_section = docs.split("## Property Tests", 1)[0]
if "Reticulum `1.4.0`" not in reference_section:
    note_error("docs/validation.md Reference Target must name Reticulum `1.4.0`")
if "RNS `1.4.0`" not in reference_section:
    note_error("docs/validation.md Reference Target must name RNS `1.4.0`")
if "rns==1.4.0" not in reference_section:
    note_error("docs/validation.md Reference Target must name the running oracle pin rns==1.4.0")
if "1.3.1" in reference_section:
    note_error("docs/validation.md Reference Target still mentions stale 1.3.1")
stale_rns_pin = "rns==" + "1.3.1"
if stale_rns_pin in ci:
    note_error(f".github/workflows/ci.yml still mentions {stale_rns_pin}")

historical_release = ".".join(str(part) for part in (1, 3, 5))
historical_benchmark_paths = {
    Path("benchmarks/CONTRIBUTING.md"),
    Path("benchmarks/implementations") / f"rns-{historical_release}.json",
    Path("benchmarks/scenarios/request-response/manifest.json"),
    Path("benchmarks/scenarios/request-response-relayed/manifest.json"),
    Path("benchmarks/scenarios/resource-bulk/manifest.json"),
}
historical_provenance_paths = {
    Path("benchmarks/src/bin/announce_verify_probe.rs"),
    Path("benchmarks/src/bin/render_results.rs"),
    Path("docs/validation.md"),
    Path("prns-core/src/engine/test_support.rs"),
    Path("prns-core/src/routing/delivery/send_group.rs"),
    Path("prns-core/src/routing/ingress/upstream_delivery.rs"),
    Path("prns-core/src/routing/links/handshake.rs"),
    Path("prns-runtime/impls/tokio/src/reactor/compression.rs"),
}
obsolete_rns = re.compile(
    r"(?i)(?:rns|reticulum)[^\n]{0,32}1\.3\.(?:5|8|9)|rns[_-]1[_-]3[_-](?:5|8|9)"
)
text_suffixes = {".json", ".jsonl", ".md", ".py", ".rs", ".sh", ".toml", ".txt", ".yaml", ".yml"}
for path in root.rglob("*"):
    if not path.is_file() or path.suffix not in text_suffixes:
        continue
    relative = path.relative_to(root)
    if any(part in {".git", "target", "validation-artifacts"} for part in relative.parts):
        continue
    if (
        relative in historical_benchmark_paths
        or relative.parts[:2] == ("benchmarks", "results")
        or (relative.parent == Path("benchmarks") and relative.name.startswith("RESULTS"))
    ):
        continue
    try:
        contents = path.read_text()
    except UnicodeDecodeError:
        continue
    for match in obsolete_rns.finditer(contents):
        line = contents.count("\n", 0, match.start()) + 1
        lines = contents.splitlines()
        context = " ".join(lines[max(0, line - 3):line + 2]).lower()
        if (
            relative in historical_provenance_paths
            and (
                (
                    "1.4.0" in context
                    and any(marker in context for marker in ("minted", "generated", "originally"))
                )
                or "historical" in context
                or (
                    relative == Path("benchmarks/src/bin/render_results.rs")
                    and "_conformance_" in context
                )
            )
        ):
            continue
        note_error(f"obsolete active RNS target at {relative}:{line}: {match.group(0)!r}")

fuzz_toml = tomllib.loads(fuzz_toml_path.read_text())
toml_fuzz_targets = {
    entry["name"]
    for entry in fuzz_toml.get("bin", [])
}
doc_fuzz_targets = set(
    re.findall(r"^cargo \+nightly fuzz run ([A-Za-z0-9_]+) --", docs, re.MULTILINE)
)
if doc_fuzz_targets != toml_fuzz_targets:
    note_error(
        "fuzz target drift: docs-only="
        + repr(sorted(doc_fuzz_targets - toml_fuzz_targets))
        + " toml-only="
        + repr(sorted(toml_fuzz_targets - doc_fuzz_targets))
    )

doc_harnesses = set(
    re.findall(r"^cargo kani -p prns-core --harness ([A-Za-z0-9_]+)$", docs, re.MULTILINE)
)
source_harnesses = set()
for source in (root / "prns-core/src").rglob("*.rs"):
    lines = source.read_text().splitlines()
    for index, line in enumerate(lines):
        if "#[kani::proof]" not in line:
            continue
        for candidate in lines[index + 1:index + 8]:
            match = re.search(r"\bfn\s+([A-Za-z0-9_]+)\s*\(", candidate)
            if match:
                source_harnesses.add(match.group(1))
                break
        else:
            note_error(f"{source.relative_to(root)} has #[kani::proof] without a nearby fn")

if doc_harnesses != source_harnesses:
    note_error(
        "Kani harness drift: docs-only="
        + repr(sorted(doc_harnesses - source_harnesses))
        + " source-only="
        + repr(sorted(source_harnesses - doc_harnesses))
    )

mutants_toml = tomllib.loads(mutants_toml_path.read_text())
for raw_path in mutants_toml.get("examine_globs", []):
    path = root / raw_path
    if not path.is_file():
        note_error(f".cargo/mutants.toml examine_globs path is missing: {raw_path}")

for command in ("cargo mutants --list-files", "cargo mutants"):
    if command not in docs:
        note_error(f"docs/validation.md no longer lists {command!r}")

for command in ("python3 scripts/oracles.py --pr", "python3 scripts/oracles.py --full", "python3 scripts/oracles.py --list"):
    if command not in docs:
        note_error(f"docs/validation.md no longer lists {command!r}")

if errors:
    for error in errors:
        print(f"VALIDATION_DOC_DRIFT: {error}", file=sys.stderr)
    sys.exit(1)

print("VALIDATION_DOC_DRIFT_OK")
PY
