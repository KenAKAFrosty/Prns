#!/usr/bin/env python3
"""Explicit host-only managed group qualifications; never part of ordinary PR runs."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
CASES = {
    "catalog": ("catalog_capacity", "scale::thousand_managed_conversations_public_create_reopen_and_live_pages", "managed-scale-result "),
    "members": ("group_only_onboarding", "member_scale::two_hundred_fifty_six_members_public_enrollment_and_late_member_links", "member-scale-result "),
    "members8": ("group_only_onboarding", "member_scale::eight_members_representative_workload", "member-scale-result "),
    "members16": ("group_only_onboarding", "member_scale::sixteen_members_representative_workload", "member-scale-result "),
    "members32": ("group_only_onboarding", "member_scale::thirty_two_members_representative_workload", "member-scale-result "),
    "members16-strict": ("group_only_onboarding", "member_scale::sixteen_strict_members_representative_workload", "member-scale-result "),
    "network8": ("group_only_onboarding", "member_scale::eight_members_four_link_messaging_journey", "member-scale-result "),
    "management8": ("group_only_onboarding", "member_scale::eight_members_management_lifecycle", "member-scale-result "),
}
PROFILE = {
    "CARGO_INCREMENTAL": "0",
    "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0",
    "CARGO_PROFILE_DEV_STRIP": "none", "CARGO_PROFILE_TEST_STRIP": "none",
    "CARGO_PROFILE_TEST_OPT_LEVEL": "2",
    "CARGO_PROFILE_TEST_DEBUG_ASSERTIONS": "true",
    "CARGO_PROFILE_TEST_OVERFLOW_CHECKS": "true",
}


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def environment(base: dict[str, str]) -> dict[str, str]:
    # GNU time localizes its labels; keep the measured RSS record parseable.
    return {**base, **PROFILE, "LC_ALL": "C"}


def timing_command(platform: str, executable: Path, test: str) -> list[str]:
    flags = {"darwin": ["-l"], "linux": ["-v"]}
    if platform not in flags:
        raise ValueError("The local private-file owner and RSS recipe support macOS/Linux only.")
    return ["/usr/bin/time", *flags[platform], str(executable), "--ignored", "--exact", test, "--nocapture", "--test-threads=1"]


def peak_rss(text: str, platform: str) -> int | None:
    pattern = r"^\s*(\d+)\s+maximum resident set size\s*$" if platform == "darwin" else r"Maximum resident set size \(kbytes\):\s*(\d+)"
    match = re.search(pattern, text, re.MULTILINE)
    return int(match.group(1)) * (1024 if platform == "linux" else 1) if match else None


def artifact(messages: list[dict], target: str) -> Path:
    paths = {message["executable"] for message in messages
             if message.get("reason") == "compiler-artifact" and message.get("executable")
             and message.get("target", {}).get("name") == target
             and "test" in message.get("target", {}).get("kind", [])
             and message.get("profile", {}).get("test") is True}
    if len(paths) != 1:
        raise ValueError(f"Expected one emitted test executable for {target}; got {sorted(paths)}")
    return Path(paths.pop()).resolve(strict=True)


def measured_result(text: str, prefix: str) -> dict:
    values = [json.loads(line.split(prefix, 1)[1]) for line in text.splitlines() if prefix in line]
    if len(values) != 1 or not isinstance(values[0], dict):
        raise ValueError("Successful qualification must emit exactly one owning JSON measurement.")
    return values[0]


def source_hashes(root: Path) -> dict[str, str]:
    # Bounded custody of the owning source/tests, not a substitute for Cargo's
    # dependency graph or a claim about every transitive compiler input.
    paths = {path for path in (root / "prns-groups").rglob("*.rs")
             if "target" not in path.relative_to(root).parts}
    paths.update(root / p for p in ("prns-groups/Cargo.toml", "prns-groups/Cargo.lock", "prns-groups/Cargo.toml", "tools/build/group-scale.py"))
    return {str(path.relative_to(root)): digest(path) for path in sorted(paths)}


def target_directory(cargo: list[str], env: dict[str, str], offline: bool) -> Path:
    command = [*cargo, "metadata", "--locked", "--no-deps", "--format-version", "1",
               "--manifest-path", str(ROOT / "prns-groups/Cargo.toml")]
    if offline:
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT, env=env, text=True))
    target = Path(metadata["target_directory"])
    if not target.is_absolute():
        raise ValueError("Cargo metadata did not return an absolute target_directory")
    return target.resolve()


def stop(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        if process.poll() is not None:
            return
        raise
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            if process.poll() is None:
                raise
        process.wait()


def guarded(command: list[str], *, cwd: Path, env: dict[str, str], stdout: Path,
            stderr: Path | None, floor: int, disk: Path,
            timeout_seconds: float | None = None) -> dict:
    initial = shutil.disk_usage(disk).free
    if initial < floor:
        raise ValueError("Disk floor prevents launch")
    started = time.monotonic()
    minimum = initial
    interrupted = False
    exhausted = False
    timed_out = False
    with stdout.open("w") as output:
        errors = stderr.open("w") if stderr else output
        try:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output, stderr=errors, start_new_session=True)
            try:
                while process.poll() is None:
                    if timeout_seconds is not None and time.monotonic() - started >= timeout_seconds:
                        timed_out = True
                        stop(process)
                        break
                    minimum = min(minimum, shutil.disk_usage(disk).free)
                    if minimum < floor:
                        exhausted = True
                        stop(process)
                        break
                    time.sleep(1)
            except KeyboardInterrupt:
                interrupted = True
                stop(process)
            finally:
                stop(process)
        finally:
            if stderr:
                errors.close()
    return {"command": command, "exit_code": process.returncode,
            "wall_seconds": time.monotonic() - started, "minimum_free_bytes": minimum,
            "disk_guard_triggered": exhausted, "runtime_limit_triggered": timed_out,
            "interrupted": interrupted or process.returncode < 0}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New private evidence directory; existing paths are refused")
    parser.add_argument("--toolchain", help="Explicit rustup toolchain; otherwise use the repository pin")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--max-runtime-seconds", type=int,
                        help="Stop only the measured test after this duration; representative cases default to 300s")
    args = parser.parse_args()
    if args.max_runtime_seconds is not None and args.max_runtime_seconds <= 0:
        parser.error("--max-runtime-seconds must be positive")
    runtime_limit = args.max_runtime_seconds
    if runtime_limit is None and args.case not in {"catalog", "members"}:
        runtime_limit = 300
    target, test, prefix = CASES[args.case]
    timing_command(sys.platform, Path("unused"), test)  # Fail before building unsupported platforms.
    if not Path("/usr/bin/time").is_file():
        parser.error("/usr/bin/time is required for isolated test-process memory measurements")
    output = args.output.expanduser().resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    env = environment(dict(os.environ))
    cargo = ["cargo", *([f"+{args.toolchain}"] if args.toolchain else [])]
    rustc = ["rustc", *([f"+{args.toolchain}"] if args.toolchain else [])]
    command = [*cargo, "test", "--locked", "--manifest-path", str(ROOT / "prns-groups/Cargo.toml"),
               "--all-features", "--test", target, "--no-run", "--message-format=json-render-diagnostics"]
    if args.offline:
        command.append("--offline")
    receipt = {"started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "case": args.case, "status": "preparing", "test": test, "profile": PROFILE,
               "compiler_flag_overrides": {key: env[key] for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER") if key in env},
               "target_directory": env.get("CARGO_TARGET_DIR", "Cargo workspace default"),
               "requested_toolchain": args.toolchain or "repository pin", "features": "all native crate features",
               "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
               "rustc": subprocess.check_output([*rustc, "--version", "--verbose"], cwd=ROOT, text=True),
               "sources": source_hashes(ROOT), "disk_floor_bytes": 5 * 1024**3,
               "runtime_limit_seconds": runtime_limit,
               "scope": "Host-only optimized test qualification with assertions/overflow checks; not release/mobile suitability, physical RF, or 256 concurrent transports"}
    def save() -> None:
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    save()
    try:
        # Guard the actual Cargo target filesystem and isolate all runtime DBs
        # below the output filesystem; no user installation is opened.
        target_disk = target_directory(cargo, env, args.offline)
        receipt["target_directory"] = str(target_disk)
        save()
        while not target_disk.exists():
            target_disk = target_disk.parent
        raw = output / "cargo.jsonl"
        receipt["build"] = guarded(command, cwd=ROOT, env=env, stdout=raw,
                                   stderr=output / "build.log", floor=receipt["disk_floor_bytes"], disk=target_disk)
        messages = [json.loads(line) for line in raw.read_text().splitlines() if line.strip()]
        with (output / "build.log").open("a") as log:
            for message in messages:
                if message.get("reason") == "compiler-message":
                    log.write(message.get("message", {}).get("rendered", ""))
        if receipt["build"]["exit_code"] != 0:
            raise ValueError("Qualification build did not complete; raw compiler evidence retained")
        executable = artifact(messages, target)
        receipt["artifact_features"] = [{"package": message["package_id"], "features": message.get("features", [])}
                                         for message in messages if message.get("reason") == "compiler-artifact"]
        raw.unlink()
        emitted = next(message for message in messages if message.get("executable")
                       and Path(message["executable"]).resolve() == executable)
        receipt["emitted_profile"] = emitted["profile"]
        if any(emitted["profile"].get(key) != value for key, value in
               {"opt_level": "2", "debug_assertions": True, "overflow_checks": True}.items()):
            raise ValueError("Emitted test profile did not retain the requested optimization/assertion policy")
        receipt["executable"] = str(executable)
        receipt["executable_sha256"] = digest(executable)
        if receipt["sources"] != source_hashes(ROOT):
            raise ValueError("Owning source changed during compilation")
        save()
        with tempfile.TemporaryDirectory(prefix="synthetic-databases-", dir=output) as temporary:
            receipt["temporary_database_root"] = temporary
            receipt["run"] = guarded(timing_command(sys.platform, executable, test), cwd=ROOT,
                                     env={**env, "TMPDIR": temporary}, stdout=output / "test.log",
                                     stderr=None, floor=receipt["disk_floor_bytes"], disk=output,
                                     timeout_seconds=runtime_limit)
        receipt["temporary_database_root_removed"] = not Path(temporary).exists()
        text = (output / "test.log").read_text()
        receipt["peak_rss_bytes"] = peak_rss(text, sys.platform)
        receipt["completed_stages"] = [json.loads(line.split("member-stage-result ", 1)[1])
                                       for line in text.splitlines() if "member-stage-result " in line]
        receipt["sources_unchanged_after_run"] = receipt["sources"] == source_hashes(ROOT)
        run = receipt["run"]
        if run["disk_guard_triggered"]:
            receipt["status"] = "disk_floor_halt"
        elif run["runtime_limit_triggered"]:
            receipt["status"] = "runtime_limit_halt"
        elif run["interrupted"]:
            receipt["status"] = "interrupted"
        elif run["exit_code"] != 0:
            receipt["status"] = "failed"
        elif receipt["peak_rss_bytes"] is None or not receipt["sources_unchanged_after_run"]:
            raise ValueError("Missing memory evidence or source changed during runtime")
        else:
            receipt["metrics"] = measured_result(text, prefix)
            receipt["status"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        stage = receipt.get("run", receipt.get("build", {}))
        receipt["status"] = ("disk_floor_halt" if stage.get("disk_guard_triggered") else
                             "runtime_limit_halt" if stage.get("runtime_limit_triggered") else
                             "interrupted" if stage.get("interrupted") else "failed")
        receipt["error"] = str(error)
    finally:
        receipt["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        receipt["logs"] = {path.name: digest(path) for path in output.iterdir() if path.name.endswith(".log")}
        save()
    print(json.dumps({"status": receipt["status"], "receipt": str(output / "receipt.json")}))
    return 0 if receipt["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
