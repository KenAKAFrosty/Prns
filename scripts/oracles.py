import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = ROOT / "oracles" / "manifest.json"
VALID_TIERS = {"pr", "full"}
VALID_KINDS = {"deterministic", "live"}


class DuplicateManifestKeyError(ValueError):
    pass


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise DuplicateManifestKeyError(f"duplicate object key {key!r}")
        value[key] = item
    return value


def load_manifest():
    try:
        manifest = json.loads(MANIFEST_PATH.read_text(), object_pairs_hook=unique_object)
    except (OSError, json.JSONDecodeError, DuplicateManifestKeyError) as error:
        raise ValueError(f"cannot load {MANIFEST_PATH.relative_to(ROOT)}: {error}") from error
    validate_manifest(manifest)
    return manifest


def validate_manifest(manifest):
    errors = []
    if manifest.get("schema") != 1:
        errors.append("schema must be 1")
    interpreters = manifest.get("interpreters")
    if not isinstance(interpreters, dict) or not interpreters:
        errors.append("interpreters must be a non-empty object")
        interpreters = {}
    for name, interpreter in interpreters.items():
        if not isinstance(interpreter, dict):
            errors.append(f"interpreter {name!r} must be an object")
            continue
        for field in ("environment", "fallback", "package"):
            if not isinstance(interpreter.get(field), str) or not interpreter[field]:
                errors.append(f"interpreter {name!r} needs a non-empty {field}")

    oracles = manifest.get("oracles")
    if not isinstance(oracles, list) or not oracles:
        errors.append("oracles must be a non-empty array")
        oracles = []
    identifiers = set()
    evidence_paths = set()
    registered_inputs = set()
    for index, oracle in enumerate(oracles):
        location = f"oracles[{index}]"
        if not isinstance(oracle, dict):
            errors.append(f"{location} must be an object")
            continue
        identifier = oracle.get("id")
        if not isinstance(identifier, str) or not identifier:
            errors.append(f"{location} needs a non-empty id")
        elif identifier in identifiers:
            errors.append(f"duplicate oracle id {identifier!r}")
        else:
            identifiers.add(identifier)
        if oracle.get("kind") not in VALID_KINDS:
            errors.append(f"{location} kind must be deterministic or live")
        tiers = oracle.get("tiers")
        if not isinstance(tiers, list) or not tiers or set(tiers) - VALID_TIERS:
            errors.append(f"{location} tiers must contain only pr or full")
        elif "full" not in tiers:
            errors.append(f"{location} must belong to the full tier")
        interpreter = oracle.get("interpreter")
        if interpreter not in interpreters:
            errors.append(f"{location} names unknown interpreter {interpreter!r}")
        command = oracle.get("command")
        if not isinstance(command, list) or not command or not all(
            isinstance(part, str) and part for part in command
        ):
            errors.append(f"{location} command must be a non-empty string array")
        timeout = oracle.get("timeout_seconds")
        if not isinstance(timeout, int) or timeout <= 0:
            errors.append(f"{location} timeout_seconds must be a positive integer")
        evidence = oracle.get("evidence")
        if not isinstance(evidence, str) or not evidence.startswith("validation-artifacts/oracles/"):
            errors.append(f"{location} evidence must live under validation-artifacts/oracles")
        elif evidence in evidence_paths:
            errors.append(f"duplicate evidence location {evidence!r}")
        else:
            evidence_paths.add(evidence)
        inputs = oracle.get("inputs")
        if not isinstance(inputs, list) or not inputs:
            errors.append(f"{location} inputs must be a non-empty array")
            continue
        for raw_path in inputs:
            if not isinstance(raw_path, str) or not raw_path:
                errors.append(f"{location} has an invalid input path")
                continue
            registered_inputs.add(raw_path)
            if not (ROOT / raw_path).is_file():
                errors.append(f"{location} input is missing: {raw_path}")

    exemptions = manifest.get("exemptions")
    if not isinstance(exemptions, list):
        errors.append("exemptions must be an array")
        exemptions = []
    exempted = set()
    for index, exemption in enumerate(exemptions):
        location = f"exemptions[{index}]"
        if not isinstance(exemption, dict):
            errors.append(f"{location} must be an object")
            continue
        path = exemption.get("path")
        reason = exemption.get("reason")
        if not isinstance(path, str) or not path:
            errors.append(f"{location} needs a non-empty path")
            continue
        if not isinstance(reason, str) or not reason:
            errors.append(f"{location} needs a non-empty reason")
        if path in registered_inputs:
            errors.append(f"{path} is both registered and exempted")
        if path in exempted:
            errors.append(f"duplicate exemption for {path}")
        exempted.add(path)
        if not (ROOT / path).is_file():
            errors.append(f"exempted path is missing: {path}")

    smoke_wrappers = {
        path.relative_to(ROOT).as_posix() for path in (ROOT / "scripts").glob("*-smoke.sh")
    }
    interop_peers = {
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "prns-core" / "tests" / "interop").glob("*.py")
    }
    inventory = smoke_wrappers | interop_peers
    missing = inventory - registered_inputs - exempted
    for path in sorted(missing):
        errors.append(f"unregistered interop asset: {path}")
    if errors:
        raise ValueError("\n".join(errors))


def selected_oracles(manifest, tier):
    return [oracle for oracle in manifest["oracles"] if tier in oracle["tiers"]]


def resolve_interpreter(manifest, name):
    specification = manifest["interpreters"][name]
    environment = specification["environment"]
    configured = os.environ.get(environment)
    candidate = configured if configured is not None else str(ROOT / specification["fallback"])
    if not candidate:
        raise ValueError(f"{environment} is set but empty")
    if os.sep in candidate:
        resolved = Path(candidate)
        if not resolved.is_absolute():
            resolved = ROOT / resolved
        if not resolved.is_file() or not os.access(resolved, os.X_OK):
            source = environment if configured is not None else "developer fallback"
            raise ValueError(f"{source} is not an executable file: {resolved}")
        return str(resolved)
    resolved = shutil.which(candidate)
    if resolved is None:
        raise ValueError(f"{environment} does not resolve to an executable: {candidate}")
    return resolved


def evidence_path(oracle):
    configured_root = Path(os.environ.get("PRNS_VALIDATION_ARTIFACTS", "validation-artifacts"))
    if not configured_root.is_absolute():
        configured_root = ROOT / configured_root
    return configured_root / "oracles" / oracle["id"]


def terminate(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return process.communicate()
    try:
        return process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        return process.communicate()


def run_oracle(manifest, oracle, interpreter):
    artifact = evidence_path(oracle)
    artifact.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    seam = manifest["interpreters"][oracle["interpreter"]]["environment"]
    environment[seam] = interpreter
    environment["PRNS_ORACLE_REQUIRED"] = "1"
    started = time.monotonic()
    timed_out = False
    exit_code = None
    spawn_error = None
    stdout = b""
    stderr = b""
    try:
        process = subprocess.Popen(
            oracle["command"],
            cwd=ROOT,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=oracle["timeout_seconds"])
        except subprocess.TimeoutExpired:
            timed_out = True
            stdout, stderr = terminate(process)
        exit_code = process.returncode
    except OSError as error:
        spawn_error = str(error)
        stderr = spawn_error.encode()
    elapsed = time.monotonic() - started
    passed = not timed_out and spawn_error is None and exit_code == 0
    (artifact / "stdout.log").write_bytes(stdout)
    (artifact / "stderr.log").write_bytes(stderr)
    result = {
        "id": oracle["id"],
        "status": "passed" if passed else "failed",
        "kind": oracle["kind"],
        "tiers": oracle["tiers"],
        "interpreter": interpreter,
        "command": oracle["command"],
        "timeout_seconds": oracle["timeout_seconds"],
        "elapsed_seconds": round(elapsed, 3),
        "exit_code": exit_code,
        "timed_out": timed_out,
        "spawn_error": spawn_error,
    }
    (artifact / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return passed, result, stdout, stderr


def list_inventory(manifest):
    print("id\ttiers\tkind\tinterpreter\ttimeout_seconds\tevidence\tcommand")
    for oracle in manifest["oracles"]:
        print(
            "\t".join(
                [
                    oracle["id"],
                    ",".join(oracle["tiers"]),
                    oracle["kind"],
                    oracle["interpreter"],
                    str(oracle["timeout_seconds"]),
                    oracle["evidence"],
                    " ".join(oracle["command"]),
                ]
            )
        )


def parse_args():
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--pr", action="store_true")
    mode.add_argument("--full", action="store_true")
    mode.add_argument("--list", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    try:
        manifest = load_manifest()
    except ValueError as error:
        print(f"ORACLE_MANIFEST_ERROR\n{error}", file=sys.stderr)
        return 2
    if args.list:
        list_inventory(manifest)
        return 0
    tier = "pr" if args.pr else "full"
    oracles = selected_oracles(manifest, tier)
    try:
        interpreters = {
            name: resolve_interpreter(manifest, name)
            for name in {oracle["interpreter"] for oracle in oracles}
        }
    except ValueError as error:
        print(f"ORACLE_INTERPRETER_ERROR {error}", file=sys.stderr)
        return 2
    print(f"ORACLE_LANE tier={tier} cases={len(oracles)}", flush=True)
    for index, oracle in enumerate(oracles, start=1):
        print(f"ORACLE_START {index}/{len(oracles)} {oracle['id']}", flush=True)
        passed, result, stdout, stderr = run_oracle(
            manifest, oracle, interpreters[oracle["interpreter"]]
        )
        if passed:
            print(
                f"ORACLE_PASS {oracle['id']} elapsed_seconds={result['elapsed_seconds']}",
                flush=True,
            )
            continue
        print(
            f"ORACLE_FAIL {oracle['id']} elapsed_seconds={result['elapsed_seconds']} "
            f"exit_code={result['exit_code']} timed_out={str(result['timed_out']).lower()}",
            file=sys.stderr,
            flush=True,
        )
        if stdout:
            sys.stderr.write(stdout.decode(errors="replace"))
        if stderr:
            sys.stderr.write(stderr.decode(errors="replace"))
        return 1
    print(f"ORACLE_LANE_OK tier={tier} cases={len(oracles)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
