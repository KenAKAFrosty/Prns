"""Target-scoped flasher hotfix identity and inherited-artifact custody."""

from __future__ import annotations

import argparse
from copy import deepcopy
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import sys

from flasher_manifest import FLASH_MANIFEST_SCHEMA, target_artifacts


HOTFIX_SCHEMA = 1
HOTFIX_METADATA_SCHEMA = 1
HOTFIX_VERSION = re.compile(r"^(.+)-hotfix\.([1-9][0-9]*)$")
TOKEN = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SURFACES = frozenset({"cli", "web"})
SPEC_FIELDS = {
    "schema",
    "release",
    "changed_boards",
    "qualification",
    "summary",
}
RELEASE_FIELDS = {
    "version",
    "base_version",
    "base_source_commit",
    "base_manifest_sha256",
    "base_release_record_sha256",
    "base_signed_candidate_sha256",
}
QUALIFICATION_FIELDS = {
    "surfaces",
    "required_scenarios",
    "required_checks",
    "physical_boards",
    "deferred_hardware",
}
DEFERRED_HARDWARE_FIELDS = {"board", "basis", "follow_up"}


@dataclass(frozen=True)
class HardwareDeferral:
    board: str
    basis: str
    follow_up: str

    def document(self) -> dict[str, str]:
        return {
            "board": self.board,
            "basis": self.basis,
            "follow_up": self.follow_up,
        }


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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


def require_version(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or value.lower() == "next"
        or PurePosixPath(value).name != value
        or any(
            character
            not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-+"
            for character in value
        )
    ):
        raise ValueError(f"{label} must be a publishable path-safe version")
    return value


def require_tokens(value: object, label: str) -> tuple[str, ...]:
    if (
        not isinstance(value, list)
        or not value
        or not all(isinstance(item, str) and TOKEN.fullmatch(item) for item in value)
        or value != sorted(set(value))
    ):
        raise ValueError(f"{label} must be a nonempty sorted unique token list")
    return tuple(value)


def require_text(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or value != value.strip()
        or not 20 <= len(value) <= 512
        or "\n" in value
        or "\r" in value
    ):
        raise ValueError(f"{label} must be 20-512 characters on one line")
    return value


def parse_deferrals(value: object) -> tuple[HardwareDeferral, ...]:
    if not isinstance(value, list):
        raise ValueError("deferred_hardware must be an array")
    parsed: list[HardwareDeferral] = []
    for index, entry in enumerate(value):
        if not isinstance(entry, dict) or set(entry) != DEFERRED_HARDWARE_FIELDS:
            raise ValueError(f"deferred_hardware[{index}] has an unsupported shape")
        board = entry.get("board")
        if not isinstance(board, str) or TOKEN.fullmatch(board) is None:
            raise ValueError(f"deferred_hardware[{index}].board must be a board token")
        parsed.append(
            HardwareDeferral(
                board=board,
                basis=require_text(
                    entry.get("basis"), f"deferred_hardware[{index}].basis"
                ),
                follow_up=require_text(
                    entry.get("follow_up"), f"deferred_hardware[{index}].follow_up"
                ),
            )
        )
    if [entry.board for entry in parsed] != sorted(
        {entry.board for entry in parsed}
    ):
        raise ValueError("deferred_hardware must be sorted and unique by board")
    return tuple(parsed)


def load_object(path: Path, label: str) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


@dataclass(frozen=True)
class HotfixSpec:
    path: Path
    version: str
    base_version: str
    base_source_commit: str
    base_manifest_sha256: str
    base_release_record_sha256: str
    base_signed_candidate_sha256: str
    changed_boards: tuple[str, ...]
    physical_boards: tuple[str, ...]
    deferred_hardware: tuple[HardwareDeferral, ...]
    surfaces: tuple[str, ...]
    required_scenarios: tuple[str, ...]
    required_checks: tuple[str, ...]
    summary: str

    @property
    def roster_version(self) -> str:
        matched = HOTFIX_VERSION.fullmatch(self.version)
        if matched is None:
            raise ValueError("hotfix release version has no suite-version prefix")
        return matched.group(1)

    def document(self) -> dict:
        return load_object(self.path, "hotfix specification")


def parse_spec(path: Path, shipping_boards: set[str] | None = None) -> HotfixSpec:
    document = load_object(path, "hotfix specification")
    if set(document) != SPEC_FIELDS or document.get("schema") != HOTFIX_SCHEMA:
        raise ValueError("hotfix specification has an unsupported shape or schema")
    release = document.get("release")
    qualification = document.get("qualification")
    if not isinstance(release, dict) or set(release) != RELEASE_FIELDS:
        raise ValueError("hotfix release identity has an unsupported shape")
    if not isinstance(qualification, dict) or set(qualification) != QUALIFICATION_FIELDS:
        raise ValueError("hotfix qualification contract has an unsupported shape")

    version = require_version(release.get("version"), "hotfix version")
    base_version = require_version(release.get("base_version"), "hotfix base version")
    matched = HOTFIX_VERSION.fullmatch(version)
    if matched is None:
        raise ValueError("hotfix version must be SUITE_VERSION-hotfix.N")
    suite_version = matched.group(1)
    sequence = int(matched.group(2))
    base_hotfix = HOTFIX_VERSION.fullmatch(base_version)
    valid_base = base_version == suite_version or (
        base_hotfix is not None
        and base_hotfix.group(1) == suite_version
        and int(base_hotfix.group(2)) < sequence
    )
    if not valid_base:
        raise ValueError(
            "hotfix base must be its suite release or an earlier hotfix in that suite"
        )
    if path.name not in {f"{version}.json", "hotfix.json"}:
        raise ValueError("hotfix specification filename must equal its release version")

    changed_boards = require_tokens(document.get("changed_boards"), "changed_boards")
    if shipping_boards is not None and not set(changed_boards) < shipping_boards:
        raise ValueError("changed_boards must be a strict subset of the shipping board set")
    surfaces = require_tokens(qualification.get("surfaces"), "qualification surfaces")
    if not set(surfaces) <= SURFACES:
        raise ValueError("hotfix qualification surfaces must be cli and/or web")
    required_scenarios = require_tokens(
        qualification.get("required_scenarios"), "required_scenarios"
    )
    required_checks = require_tokens(
        qualification.get("required_checks"), "required_checks"
    )
    physical_boards = require_tokens(
        qualification.get("physical_boards"), "physical_boards"
    )
    deferred_hardware = parse_deferrals(qualification.get("deferred_hardware"))
    deferred_boards = {entry.board for entry in deferred_hardware}
    if set(physical_boards) & deferred_boards:
        raise ValueError("physical_boards and deferred_hardware must be disjoint")
    if set(physical_boards) | deferred_boards != set(changed_boards):
        raise ValueError(
            "physical_boards and deferred_hardware must exactly partition changed_boards"
        )
    summary = require_text(document.get("summary"), "hotfix summary")

    return HotfixSpec(
        path=path,
        version=version,
        base_version=base_version,
        base_source_commit=require_commit(
            release.get("base_source_commit"), "hotfix base source commit"
        ),
        base_manifest_sha256=require_sha256(
            release.get("base_manifest_sha256"), "hotfix base manifest SHA-256"
        ),
        base_release_record_sha256=require_sha256(
            release.get("base_release_record_sha256"),
            "hotfix base release-record SHA-256",
        ),
        base_signed_candidate_sha256=require_sha256(
            release.get("base_signed_candidate_sha256"),
            "hotfix base signed-candidate SHA-256",
        ),
        changed_boards=changed_boards,
        physical_boards=physical_boards,
        deferred_hardware=deferred_hardware,
        surfaces=surfaces,
        required_scenarios=required_scenarios,
        required_checks=required_checks,
        summary=summary,
    )


def spec_path(repository: Path, version: str) -> Path:
    require_version(version, "release version")
    return repository / "release" / "flash" / "hotfixes" / f"{version}.json"


def load_spec(repository: Path, version: str, shipping_boards: set[str] | None = None) -> HotfixSpec:
    path = spec_path(repository, version)
    if not path.is_file():
        raise ValueError(f"release version differs from VERSION and has no hotfix spec: {path}")
    return parse_spec(path, shipping_boards)


def resolve_release_identity(repository: Path, requested_version: str | None) -> tuple[str, HotfixSpec | None]:
    suite_version = require_version(
        (repository / "VERSION").read_text(encoding="utf-8").strip(),
        "repository VERSION",
    )
    version = requested_version.strip() if requested_version is not None else suite_version
    version = require_version(version, "requested flasher version")
    if version == suite_version:
        return version, None
    spec = load_spec(repository, version)
    if spec.roster_version != suite_version:
        raise ValueError("hotfix version prefix must equal the repository suite VERSION")
    return version, spec


def artifact_entries(target: dict) -> list[dict]:
    return target_artifacts(target)


def target_map(manifest: dict, label: str) -> dict[str, dict]:
    if manifest.get("schema") != FLASH_MANIFEST_SCHEMA:
        raise ValueError(f"{label} must use flash manifest schema {FLASH_MANIFEST_SCHEMA}")
    targets = manifest.get("targets")
    if not isinstance(targets, list) or not all(
        isinstance(target, dict) and isinstance(target.get("board_slug"), str)
        for target in targets
    ):
        raise ValueError(f"{label} targets are malformed")
    mapped = {target["board_slug"]: target for target in targets}
    if len(mapped) != len(targets):
        raise ValueError(f"{label} duplicates a board target")
    return mapped


def rewritten_target(target: dict, base_version: str, version: str) -> dict:
    rewritten = deepcopy(target)
    board = rewritten.get("board_slug")
    if not isinstance(board, str):
        raise ValueError("baseline target has no board slug")
    base_prefix = f"firmware/hopspot/{board}/{base_version}/"
    version_prefix = f"firmware/hopspot/{board}/{version}/"
    for artifact in artifact_entries(rewritten):
        path = artifact.get("path")
        if not isinstance(path, str) or not path.startswith(base_prefix):
            raise ValueError(f"baseline artifact path is outside {base_prefix}")
        artifact["path"] = version_prefix + path.removeprefix(base_prefix)
    return rewritten


def source_capability(board: str) -> dict:
    return {
        "schema": 1,
        "board_slug": board,
        "nominally_capable": False,
        "status": "absent",
        "source": None,
        "reserve_bytes": None,
    }


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def history_identity(history: Path, spec: HotfixSpec) -> tuple[dict, Path]:
    metadata = load_object(history / "history.json", "release-history metadata")
    head = metadata.get("head")
    expected = {
        "version": spec.base_version,
        "source_commit": spec.base_source_commit,
        "manifest_sha256": spec.base_manifest_sha256,
        "release_record_sha256": spec.base_release_record_sha256,
        "signed_bundle_sha256": spec.base_signed_candidate_sha256,
    }
    if metadata.get("schema") != 1 or metadata.get("mode") != "retained" or head != expected:
        raise ValueError("verified release-history head differs from the hotfix base identity")
    base_root = history / "releases" / spec.base_version
    manifest_path = base_root / "flash-manifest.json"
    if sha256(manifest_path) != spec.base_manifest_sha256:
        raise ValueError("retained base manifest differs from the hotfix specification")
    if not (base_root / "flash-manifest.json.minisig").is_file():
        raise ValueError("retained hotfix base manifest signature is missing")
    return load_object(manifest_path, "retained base manifest"), base_root


def compose(repository: Path, history: Path, candidate: Path, version: str) -> dict:
    spec = load_spec(repository, version)
    baseline, base_root = history_identity(history, spec)
    baseline_release = baseline.get("release")
    if baseline_release != {
        "version": spec.base_version,
        "channel": "stable",
        "commit": spec.base_source_commit,
    }:
        raise ValueError("retained base manifest release identity differs from the hotfix spec")
    baseline_targets = target_map(baseline, "retained base manifest")
    shipping_boards = set(baseline_targets)
    spec = parse_spec(spec.path, shipping_boards)

    inherited_boards = tuple(sorted(shipping_boards - set(spec.changed_boards)))
    inherited_artifacts = []
    for board in inherited_boards:
        target = rewritten_target(
            baseline_targets[board], spec.base_version, spec.version
        )
        output = candidate / "firmware" / "hopspot" / board / spec.version
        if output.exists():
            raise ValueError(f"inherited board output already exists: {output}")
        output.mkdir(parents=True)
        base_artifacts = artifact_entries(baseline_targets[board])
        current_artifacts = artifact_entries(target)
        for base_artifact, current_artifact in zip(base_artifacts, current_artifacts, strict=True):
            base_path = base_artifact["path"]
            current_path = current_artifact["path"]
            source = base_root / base_path
            destination = candidate / current_path
            if (
                not source.is_file()
                or source.stat().st_size != base_artifact.get("size")
                or sha256(source) != base_artifact.get("sha256")
            ):
                raise ValueError(f"retained base artifact differs from its manifest: {base_path}")
            if destination.parent != output:
                raise ValueError("rewritten inherited artifact escaped its board output")
            shutil.copy2(source, destination)
            inherited_artifacts.append(
                {
                    "board": board,
                    "base_path": base_path,
                    "path": current_path,
                    "size": source.stat().st_size,
                    "sha256": sha256(source),
                }
            )
        write_json(output / "target.json", target)
        write_json(output / "source-capability.json", source_capability(board))

    for board in spec.changed_boards:
        output = candidate / "firmware" / "hopspot" / board / spec.version
        if not (output / "target.json").is_file() or not (
            output / "source-capability.json"
        ).is_file():
            raise ValueError(f"changed board was not freshly built: {board}")

    metadata = {
        "schema": HOTFIX_METADATA_SCHEMA,
        "release": {
            "version": spec.version,
            "base_version": spec.base_version,
            "base_source_commit": spec.base_source_commit,
            "base_manifest_sha256": spec.base_manifest_sha256,
            "base_release_record_sha256": spec.base_release_record_sha256,
            "base_signed_candidate_sha256": spec.base_signed_candidate_sha256,
        },
        "changed_boards": list(spec.changed_boards),
        "inherited_boards": list(inherited_boards),
        "inherited_artifacts": sorted(
            inherited_artifacts, key=lambda artifact: artifact["path"]
        ),
        "qualification": {
            "surfaces": list(spec.surfaces),
            "required_scenarios": list(spec.required_scenarios),
            "required_checks": list(spec.required_checks),
            "physical_boards": list(spec.physical_boards),
            "deferred_hardware": [
                deferral.document() for deferral in spec.deferred_hardware
            ],
        },
        "summary": spec.summary,
    }
    write_json(candidate / "metadata" / "hotfix.json", metadata)
    return metadata


def verify_candidate(repository: Path, candidate: Path) -> HotfixSpec | None:
    metadata_path = candidate / "metadata" / "hotfix.json"
    candidate_spec = candidate / "qualification" / "hotfix.json"
    if not metadata_path.exists() and not candidate_spec.exists():
        return None
    if not metadata_path.is_file() or not candidate_spec.is_file():
        raise ValueError("hotfix candidate must carry both its specification and metadata")
    version = (candidate / "VERSION").read_text(encoding="utf-8").strip()

    manifest = load_object(candidate / "flash-manifest.json", "hotfix manifest")
    targets = target_map(manifest, "hotfix manifest")
    spec = parse_spec(candidate_spec, set(targets))
    if spec.version != version:
        raise ValueError("candidate hotfix spec differs from candidate VERSION")
    committed_spec = spec_path(repository, version)
    if committed_spec.is_file() and candidate_spec.read_bytes() != committed_spec.read_bytes():
        raise ValueError("candidate hotfix specification differs from its committed source")
    metadata = load_object(metadata_path, "hotfix metadata")
    baseline_path = (
        candidate
        / "website"
        / "releases"
        / spec.base_version
        / "flash-manifest.json"
    )
    if sha256(baseline_path) != spec.base_manifest_sha256:
        raise ValueError("candidate retained base manifest differs from the hotfix spec")
    baseline = load_object(baseline_path, "candidate retained base manifest")
    baseline_targets = target_map(baseline, "candidate retained base manifest")
    if set(baseline_targets) != set(targets):
        raise ValueError("hotfix and base manifests have different shipping board sets")

    inherited_boards = tuple(sorted(set(targets) - set(spec.changed_boards)))
    inherited_artifacts = []
    changed_artifact_boards: set[str] = set()
    for board, target in targets.items():
        expected = rewritten_target(
            baseline_targets[board], spec.base_version, spec.version
        )
        if board in inherited_boards:
            if target != expected:
                raise ValueError(f"inherited target metadata changed for {board}")
        else:
            base_identities = {
                (artifact.get("size"), artifact.get("sha256"))
                for artifact in artifact_entries(baseline_targets[board])
            }
            current_identities = {
                (artifact.get("size"), artifact.get("sha256"))
                for artifact in artifact_entries(target)
            }
            if current_identities != base_identities:
                changed_artifact_boards.add(board)
        for artifact in artifact_entries(target):
            path = candidate / artifact["path"]
            if (
                not path.is_file()
                or path.stat().st_size != artifact.get("size")
                or sha256(path) != artifact.get("sha256")
            ):
                raise ValueError(f"hotfix artifact differs from its manifest: {artifact['path']}")
        if board in inherited_boards:
            for base_artifact, current_artifact in zip(
                artifact_entries(baseline_targets[board]),
                artifact_entries(target),
                strict=True,
            ):
                inherited_artifacts.append(
                    {
                        "board": board,
                        "base_path": base_artifact["path"],
                        "path": current_artifact["path"],
                        "size": current_artifact["size"],
                        "sha256": current_artifact["sha256"],
                    }
                )
    unchanged_boards = sorted(set(spec.changed_boards) - changed_artifact_boards)
    if unchanged_boards:
        raise ValueError(
            f"hotfix does not change an artifact for declared boards: {unchanged_boards}"
        )

    expected_metadata = {
        "schema": HOTFIX_METADATA_SCHEMA,
        "release": {
            "version": spec.version,
            "base_version": spec.base_version,
            "base_source_commit": spec.base_source_commit,
            "base_manifest_sha256": spec.base_manifest_sha256,
            "base_release_record_sha256": spec.base_release_record_sha256,
            "base_signed_candidate_sha256": spec.base_signed_candidate_sha256,
        },
        "changed_boards": list(spec.changed_boards),
        "inherited_boards": list(inherited_boards),
        "inherited_artifacts": sorted(
            inherited_artifacts, key=lambda artifact: artifact["path"]
        ),
        "qualification": {
            "surfaces": list(spec.surfaces),
            "required_scenarios": list(spec.required_scenarios),
            "required_checks": list(spec.required_checks),
            "physical_boards": list(spec.physical_boards),
            "deferred_hardware": [
                deferral.document() for deferral in spec.deferred_hardware
            ],
        },
        "summary": spec.summary,
    }
    if metadata != expected_metadata:
        raise ValueError("hotfix metadata differs from the recomputed inheritance contract")
    return spec


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    identity = subparsers.add_parser("identity")
    identity.add_argument("--repository", type=Path, default=Path.cwd())
    identity.add_argument("--version")
    identity.add_argument(
        "--format",
        choices=(
            "json",
            "version",
            "base-version",
            "roster-version",
            "changed-boards",
        ),
        default="json",
    )
    compose_parser = subparsers.add_parser("compose")
    compose_parser.add_argument("--repository", type=Path, default=Path.cwd())
    compose_parser.add_argument("--history", type=Path, required=True)
    compose_parser.add_argument("--candidate", type=Path, required=True)
    compose_parser.add_argument("--version", required=True)
    verify = subparsers.add_parser("verify-candidate")
    verify.add_argument("--repository", type=Path, default=Path.cwd())
    verify.add_argument("--candidate", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "identity":
            version, spec = resolve_release_identity(
                arguments.repository.resolve(), arguments.version
            )
            value = {
                "version": version,
                "kind": "hotfix" if spec is not None else "release",
                "suite_version": (
                    arguments.repository.resolve() / "VERSION"
                ).read_text(encoding="utf-8").strip(),
                "base_version": spec.base_version if spec is not None else None,
                "changed_boards": list(spec.changed_boards) if spec is not None else [],
            }
            if arguments.format == "json":
                print(json.dumps(value, sort_keys=True))
            elif arguments.format == "version":
                print(version)
            elif arguments.format == "base-version":
                print(spec.base_version if spec is not None else version)
            elif arguments.format == "roster-version":
                print(spec.roster_version if spec is not None else version)
            else:
                for board in spec.changed_boards if spec is not None else ():
                    print(board)
        elif arguments.command == "compose":
            metadata = compose(
                arguments.repository.resolve(),
                arguments.history.resolve(),
                arguments.candidate.resolve(),
                arguments.version,
            )
            print(json.dumps(metadata, sort_keys=True))
        else:
            spec = verify_candidate(
                arguments.repository.resolve(), arguments.candidate.resolve()
            )
            print(
                "verified target-scoped hotfix inheritance"
                if spec is not None
                else "verified ordinary flasher release identity"
            )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"flasher hotfix validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
