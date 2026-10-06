"""Run the full libtest inventory in bounded, independent Miri processes."""
from __future__ import annotations

import json
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator


WORKERS_PER_MODEL = 4


class MiriExecutionError(RuntimeError):
    pass


@dataclass(frozen=True)
class TestExecution:
    name: str
    returncode: int
    output: bytes


def discover_tests(
    command: tuple[str, ...], filters: tuple[str, ...], cwd: Path, environment: dict[str, str],
) -> tuple[str, ...]:
    tests: list[str] = []
    seen: set[str] = set()
    for test_filter in filters:
        result = subprocess.run(
            (*command, "--lib", "--", test_filter, "--list", "--format", "terse", "--color", "never"),
            cwd=cwd, env=environment, capture_output=True, check=True,
        )
        selected = {
            line.removesuffix(": test")
            for line in result.stdout.decode("utf-8").splitlines()
            if line.endswith(": test")
        }
        if not selected:
            raise MiriExecutionError(f"Miri filter {test_filter!r} discovered no unit tests")
        tests.extend(sorted(selected - seen))
        seen.update(selected)
    return tuple(tests)


def execute_test(
    command: tuple[str, ...], name: str, cwd: Path, environment: dict[str, str], log: Path,
) -> TestExecution:
    started = time.monotonic()
    print(f"[embedded-miri] start {name}; log={log}", flush=True)
    with log.open("wb") as output:
        output.write(f"test={name}\n".encode())
        output.flush()
        result = subprocess.run(
            (*command, "--lib", "--", name, "--exact", "--test-threads=1", "--color", "never"),
            cwd=cwd, env=environment, stdout=output, stderr=subprocess.STDOUT,
        )
    print(
        f"[embedded-miri] finish {name}; exit={result.returncode}; "
        f"seconds={time.monotonic() - started:.3f}; log={log}",
        flush=True,
    )
    return TestExecution(name, result.returncode, log.read_bytes())


def run_tests(
    command: tuple[str, ...], names: tuple[str, ...], cwd: Path,
    environment: dict[str, str], directory: Path, *, workers: int = WORKERS_PER_MODEL,
) -> Iterator[TestExecution]:
    if not names or len(names) != len(set(names)):
        raise MiriExecutionError("full Miri execution requires a nonempty unique test inventory")
    if type(workers) is not int or not 1 <= workers <= WORKERS_PER_MODEL:
        raise MiriExecutionError(f"Miri requires between one and {WORKERS_PER_MODEL} workers per model")
    directory.mkdir(parents=True, exist_ok=True)
    for previous in directory.glob("[0-9][0-9][0-9][0-9].log"):
        previous.unlink()
    (directory / "tests.json").write_text(json.dumps(names, indent=2) + "\n")
    with ThreadPoolExecutor(max_workers=workers) as executor:
        pending = [
            executor.submit(
                execute_test, command, name, cwd, environment, directory / f"{index:04}.log",
            )
            for index, name in enumerate(names)
        ]
        for future in as_completed(pending):
            yield future.result()
