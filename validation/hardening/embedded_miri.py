from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from enum import Enum
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
INVENTORY_PATH = ROOT / "validation" / "hardening" / "embedded-miri.toml"
RUNNER_PATH = ROOT / "validation" / "hardening" / "embedded_miri.py"
VALID_IDENTIFIER = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,78}[a-z0-9])?")
TEST_RESULT = re.compile(
    rb"test result: ok\. (?P<passed>[0-9]+) passed; 0 failed; [0-9]+ ignored;"
)


class Mode(Enum):
    QUICK = "quick"
    FULL = "full"


class BorrowModel(Enum):
    STACKED = "stacked"
    TREE = "tree"


@dataclass(frozen=True)
class Scenario:
    component: str
    identifier: str
    manifest: Path
    features: tuple[str, ...]
    quick_filters: tuple[str, ...]
    full_filters: tuple[str, ...]
    sources: tuple[Path, ...]

    def filters(self, mode: Mode) -> tuple[str, ...]:
        match mode:
            case Mode.QUICK:
                return self.quick_filters
            case Mode.FULL:
                return self.full_filters


@dataclass(frozen=True)
class Observation:
    model: BorrowModel
    completed_tests: int
    log: Path


@dataclass(frozen=True)
class ToolchainIdentity:
    channel: str
    rustc_version: str
    miri_version: str


@dataclass(frozen=True)
class ModeConfiguration:
    borrow_models: tuple[BorrowModel, ...]
    coverage: str
    runner: str


class EmbeddedMiriError(RuntimeError):
    pass


def load_inventory(path: Path = INVENTORY_PATH) -> tuple[Scenario, ...]:
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise EmbeddedMiriError(f"cannot load {relative(path)}: {error}") from error
    if document.get("schema") != 1:
        raise EmbeddedMiriError("embedded Miri inventory schema must be 1")
    scenarios = tuple(parse_scenario(entry) for entry in document.get("scenario", []))
    if not scenarios:
        raise EmbeddedMiriError("embedded Miri inventory is empty")
    components = [scenario.component for scenario in scenarios]
    if len(components) != len(set(components)):
        raise EmbeddedMiriError("embedded Miri inventory repeats a component")
    return scenarios


def parse_scenario(entry: object) -> Scenario:
    if not isinstance(entry, dict):
        raise EmbeddedMiriError("embedded Miri scenario must be a table")
    component = identifier(entry.get("component"), "component")
    scenario_id = identifier(entry.get("id"), "scenario")
    manifest = repository_file(entry.get("manifest"), "manifest")
    features = string_tuple(entry.get("features"), "features")
    quick_filters = string_tuple(entry.get("quick_filters"), "quick_filters")
    full_filters = string_tuple(entry.get("full_filters"), "full_filters")
    sources = tuple(
        repository_path(value, "source") for value in string_tuple(entry.get("sources"), "sources")
    )
    return Scenario(
        component=component,
        identifier=scenario_id,
        manifest=manifest,
        features=features,
        quick_filters=quick_filters,
        full_filters=full_filters,
        sources=sources,
    )


def identifier(value: object, kind: str) -> str:
    if not isinstance(value, str) or not VALID_IDENTIFIER.fullmatch(value):
        raise EmbeddedMiriError(f"invalid embedded Miri {kind} identifier {value!r}")
    return value


def string_tuple(value: object, name: str) -> tuple[str, ...]:
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item for item in value)
        or len(value) != len(set(value))
    ):
        raise EmbeddedMiriError(f"embedded Miri {name} must contain unique strings")
    return tuple(value)


def repository_path(value: object, kind: str) -> Path:
    if not isinstance(value, str) or not value:
        raise EmbeddedMiriError(f"embedded Miri {kind} path is invalid")
    path = ROOT / value
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise EmbeddedMiriError(f"embedded Miri {kind} path is missing: {value}") from error
    if ROOT not in resolved.parents:
        raise EmbeddedMiriError(f"embedded Miri {kind} path escapes the repository: {value}")
    return resolved


def repository_file(value: object, kind: str) -> Path:
    path = repository_path(value, kind)
    if not path.is_file():
        raise EmbeddedMiriError(f"embedded Miri {kind} is not a file: {relative(path)}")
    return path


def mode_configuration(mode: Mode) -> ModeConfiguration:
    match mode:
        case Mode.QUICK:
            return ModeConfiguration(
                borrow_models=(BorrowModel.STACKED,),
                coverage="stacked",
                runner="miri-stacked",
            )
        case Mode.FULL:
            return ModeConfiguration(
                borrow_models=(BorrowModel.STACKED, BorrowModel.TREE),
                coverage="stacked-and-tree",
                runner="miri-stacked-tree",
            )


def nightly_toolchain() -> str:
    manifest = tomllib.loads((ROOT / "validation" / "manifest.toml").read_text(encoding="utf-8"))
    value = manifest.get("toolchains", {}).get("nightly")
    if not isinstance(value, str) or not re.fullmatch(r"nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}", value):
        raise EmbeddedMiriError("validation nightly toolchain is not exact-pinned")
    return value


def prepare_miri(toolchain: str) -> None:
    try:
        available = subprocess.run(
            ("rustup", "run", toolchain, "rustc", "--version"),
            cwd=ROOT,
            capture_output=True,
        )
        if available.returncode != 0:
            subprocess.run(
                ("rustup", "toolchain", "install", toolchain, "--profile", "minimal"),
                cwd=ROOT,
                check=True,
            )
        installed = subprocess.run(
            ("rustup", "component", "list", "--toolchain", toolchain, "--installed"),
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        has_miri = any(line == "miri" or line.startswith("miri-") for line in installed)
        has_rust_src = any(
            line == "rust-src" or line.startswith("rust-src-") for line in installed
        )
        if not has_miri or not has_rust_src:
            subprocess.run(
                (
                    "rustup",
                    "component",
                    "add",
                    "--toolchain",
                    toolchain,
                    "miri",
                    "rust-src",
                ),
                cwd=ROOT,
                check=True,
            )
        subprocess.run(
            ("cargo", f"+{toolchain}", "miri", "setup"), cwd=ROOT, check=True
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise EmbeddedMiriError(
            "could not prepare the pinned Miri toolchain; run "
            "./tools/prns doctor embedded-assurance"
        ) from error


def command_for(
    scenario: Scenario,
    test_filter: str,
    toolchain: str,
) -> tuple[str, ...]:
    command = [
        "cargo",
        f"+{toolchain}",
        "miri",
        "test",
        "--locked",
        "--manifest-path",
        str(scenario.manifest),
    ]
    if scenario.features:
        command.extend(("--features", ",".join(scenario.features)))
    command.extend(("--", test_filter, "--test-threads=1"))
    return tuple(command)


def miri_flags(model: BorrowModel) -> tuple[str, ...]:
    match model:
        case BorrowModel.STACKED:
            return ()
        case BorrowModel.TREE:
            return ("-Zmiri-tree-borrows",)


def miri_environment(model: BorrowModel) -> dict[str, str]:
    environment = os.environ.copy()
    environment["MIRIFLAGS"] = " ".join(miri_flags(model))
    return environment


def render_log(
    model: BorrowModel,
    identity: ToolchainIdentity,
    outputs: list[bytes],
) -> bytes:
    header = (
        f"borrow-model={model.value}\n"
        f"miri-flags={json.dumps(miri_flags(model), separators=(',', ':'))}\n"
        f"toolchain={identity.channel}\n"
        f"rustc={identity.rustc_version}\n"
        f"miri={identity.miri_version}\n"
    ).encode()
    if not outputs:
        return header
    return header + b"\n" + b"\n".join(outputs)


def run_scenario(
    scenario: Scenario,
    mode: Mode,
    model: BorrowModel,
    identity: ToolchainIdentity,
    artifact_directory: Path,
) -> Observation:
    log = artifact_directory / f"{scenario.component}-{model.value}.log"
    completed_tests = 0
    outputs = []
    successful_outputs = []
    failed_commands = []
    for test_filter in scenario.filters(mode):
        command = command_for(scenario, test_filter, identity.channel)
        environment = miri_environment(model)
        print(f"[embedded-miri:{model.value}] {' '.join(command)}", flush=True)
        result = subprocess.run(command, cwd=ROOT, env=environment, capture_output=True)
        output = result.stderr + result.stdout
        outputs.append(output)
        sys.stdout.buffer.write(output)
        sys.stdout.flush()
        if result.returncode != 0:
            failed_commands.append(test_filter)
            continue
        successful_outputs.append(output)
    log.write_bytes(render_log(model, identity, outputs))
    if failed_commands:
        joined = ", ".join(failed_commands)
        raise EmbeddedMiriError(
            f"{scenario.component} {model.value} failed filters: {joined}; log={relative(log)}"
        )
    for output in successful_outputs:
        completed_tests += parse_completed_tests(output)
    if completed_tests == 0:
        raise EmbeddedMiriError(f"{scenario.component} {model.value} completed no tests")
    return Observation(model=model, completed_tests=completed_tests, log=log)


def parse_completed_tests(output: bytes) -> int:
    match = TEST_RESULT.search(output)
    if match is None:
        raise EmbeddedMiriError("Miri output has no successful unit-test result")
    return int(match.group("passed"))


def tool_version(command: tuple[str, ...]) -> str:
    result = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True)
    output = (result.stdout or result.stderr).strip()
    if not output:
        raise EmbeddedMiriError(f"tool returned an empty identity: {' '.join(command)}")
    return output.splitlines()[0]


def record_proof(
    scenario: Scenario,
    configuration: ModeConfiguration,
    observations: tuple[Observation, ...],
    artifact_directory: Path,
    identity: ToolchainIdentity,
) -> None:
    completed = {observation.completed_tests for observation in observations}
    if len(completed) != 1:
        raise EmbeddedMiriError(
            f"{scenario.component} borrow models completed different test inventories: {sorted(completed)}"
        )
    output = artifact_directory / f"{scenario.component}.assurance.json"
    command = [
        str(ROOT / "tools" / "prns"),
        "build",
        "embedded",
        "assurance",
        "record",
        "miri",
        "--component",
        scenario.component,
        "--scenario",
        scenario.identifier,
        "--runner",
        configuration.runner,
        "--coverage",
        configuration.coverage,
        "--completed-tests",
        str(next(iter(completed))),
        "--rustc-version",
        identity.rustc_version,
        "--miri-version",
        identity.miri_version,
        "--output",
        str(output),
    ]
    for source in (INVENTORY_PATH, RUNNER_PATH, *scenario.sources):
        command.extend(("--source", str(source)))
    for observation in observations:
        command.extend(("--log", str(observation.log)))
    subprocess.run(command, cwd=ROOT, check=True)


def artifact_directory() -> Path:
    artifact_root = Path(
        os.environ.get("PRNS_VALIDATION_ARTIFACT_ROOT", ROOT / "validation-artifacts")
    ).resolve()
    suite = os.environ.get("PRNS_VALIDATION_SUITE", "embedded-miri-development")
    artifact_directory = Path(
        os.environ.get(
            "PRNS_VALIDATION_ARTIFACT_DIR",
            artifact_root / "results" / suite,
        )
    ).resolve()
    if artifact_root != artifact_directory and artifact_root not in artifact_directory.parents:
        raise EmbeddedMiriError("embedded Miri artifact directory is outside its artifact root")
    artifact_directory.mkdir(parents=True, exist_ok=True)
    return artifact_directory


def clear_owned_artifacts(scenarios: tuple[Scenario, ...], artifact_directory: Path) -> None:
    for scenario in scenarios:
        for suffix in (".assurance.json", "-stacked.log", "-tree.log"):
            path = artifact_directory / f"{scenario.component}{suffix}"
            if path.is_file() or path.is_symlink():
                path.unlink()


def relative(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def run(mode: Mode) -> None:
    scenarios = load_inventory()
    configuration = mode_configuration(mode)
    artifacts = artifact_directory()
    clear_owned_artifacts(scenarios, artifacts)
    toolchain = nightly_toolchain()
    prepare_miri(toolchain)
    identity = ToolchainIdentity(
        channel=toolchain,
        rustc_version=tool_version(("rustc", f"+{toolchain}", "--version")),
        miri_version=tool_version(("cargo", f"+{toolchain}", "miri", "--version")),
    )
    failures = []
    for scenario in scenarios:
        observations = []
        for model in configuration.borrow_models:
            try:
                observations.append(
                    run_scenario(
                        scenario,
                        mode,
                        model,
                        identity,
                        artifacts,
                    )
                )
            except (EmbeddedMiriError, OSError, subprocess.SubprocessError) as error:
                failures.append(str(error))
        if len(observations) != len(configuration.borrow_models):
            continue
        record_proof(
            scenario,
            configuration,
            tuple(observations),
            artifacts,
            identity,
        )
    if failures:
        raise EmbeddedMiriError("\n".join(failures))
    print(
        f"EMBEDDED_MIRI_OK mode={mode.value} scenarios={len(scenarios)} "
        f"models={len(configuration.borrow_models)}"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=[mode.value for mode in Mode])
    return parser


def main() -> int:
    arguments = build_parser().parse_args()
    try:
        run(Mode(arguments.mode))
        return 0
    except (EmbeddedMiriError, OSError, subprocess.SubprocessError) as error:
        print(f"EMBEDDED_MIRI_ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
