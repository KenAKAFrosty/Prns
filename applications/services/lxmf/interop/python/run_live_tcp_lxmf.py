#!/usr/bin/env python3
"""Run the pinned Python/native Prns two-way direct LXMF gate."""

from __future__ import annotations

import os
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping
from pathlib import Path

from tcp_fixture import (
    DEFAULT_LISTEN_IP,
    EXPECTED_DESTINATION_ENV,
    LISTEN_IP_ENV,
    OUTBOUND_DELAY_ENV,
    RUST_OBSERVED_MARKER,
    WILDCARD_OPT_IN_ENV,
)


APPLICATIONS_ROOT = Path(__file__).resolve().parents[4]
PEER = Path(__file__).with_name("live_tcp_lxmf_peer.py")
MANIFEST = APPLICATIONS_ROOT / "services/lxmf/Cargo.toml"
READY = "PINNED_PYTHON_LXMF_UP"
PYTHON_SUCCESS = "PINNED_PYTHON_LXMF_OK inbound=verified outbound=proof links=two"
RUST_SUCCESS = "PRNS_LXMF_LIVE_OK inbound=verified outbound=proof links=two"
START_TIMEOUT_SECONDS = 15.0
EXCHANGE_TIMEOUT_SECONDS = 60.0


def host_environment(inherited: Mapping[str, str], work: Path, port: int) -> dict[str, str]:
    environment = dict(inherited)
    environment["PYTHONIOENCODING"] = "utf-8:strict"
    environment["PRNS_LXMF_TCP_PORT"] = str(port)
    environment["PRNS_LXMF_CONFIG_DIR"] = str(work / "rns")
    environment[LISTEN_IP_ENV] = DEFAULT_LISTEN_IP
    environment.pop(WILDCARD_OPT_IN_ENV, None)
    # The isolated Rust peer has its own destination, not a physical phone's.
    environment.pop(EXPECTED_DESTINATION_ENV, None)
    environment.pop(OUTBOUND_DELAY_ENV, None)
    return environment


def available_port() -> int:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    listener.close()
    return port


def read_log(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return ""


def stop(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def wait_for_marker(
    process: subprocess.Popen[bytes],
    log_path: Path,
    marker: str,
    deadline: float,
) -> None:
    while time.monotonic() < deadline:
        output = read_log(log_path)
        if marker in output:
            return
        status = process.poll()
        if status is not None:
            raise RuntimeError(
                f"peer exited with status {status} before {marker!r}\n{output}"
            )
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for {marker!r}\n{read_log(log_path)}")


def main() -> int:
    python_process: subprocess.Popen[bytes] | None = None
    rust_process: subprocess.Popen[bytes] | None = None
    with tempfile.TemporaryDirectory(prefix="prns-lxmf-live-") as temporary:
        work = Path(temporary)
        python_log = work / "python.log"
        rust_log = work / "rust.log"
        port = available_port()
        environment = host_environment(os.environ, work, port)
        try:
            with python_log.open("wb") as output:
                python_process = subprocess.Popen(
                    [sys.executable, str(PEER)],
                    cwd=APPLICATIONS_ROOT,
                    env=environment,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                )
            wait_for_marker(
                python_process,
                python_log,
                READY,
                time.monotonic() + START_TIMEOUT_SECONDS,
            )

            rust_environment = environment.copy()
            rust_environment["PRNS_LXMF_TCP_TARGET"] = f"127.0.0.1:{port}"
            with rust_log.open("wb") as output:
                rust_process = subprocess.Popen(
                    [
                        "cargo",
                        "run",
                        "--quiet",
                        "--locked",
                        "--manifest-path",
                        str(MANIFEST),
                        "--features",
                        "tokio-host",
                        "--example",
                        "live_tcp_peer",
                    ],
                    cwd=APPLICATIONS_ROOT,
                    env=rust_environment,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                )

            deadline = time.monotonic() + EXCHANGE_TIMEOUT_SECONDS
            while time.monotonic() < deadline:
                python_status = python_process.poll()
                rust_status = rust_process.poll()
                if python_status is not None and rust_status is not None:
                    break
                if python_status not in (None, 0) or rust_status not in (None, 0):
                    break
                time.sleep(0.05)

            python_output = read_log(python_log)
            rust_output = read_log(rust_log)
            if python_process.poll() is None or rust_process.poll() is None:
                raise RuntimeError(
                    "live exchange timed out\n"
                    f"--- pinned Python ---\n{python_output}"
                    f"--- native Prns ---\n{rust_output}"
                )
            if python_process.returncode != 0 or rust_process.returncode != 0:
                raise RuntimeError(
                    "live exchange peer failed\n"
                    f"--- pinned Python ({python_process.returncode}) ---\n{python_output}"
                    f"--- native Prns ({rust_process.returncode}) ---\n{rust_output}"
                )
            if (
                RUST_OBSERVED_MARKER not in python_output
                or PYTHON_SUCCESS not in python_output
                or RUST_SUCCESS not in rust_output
            ):
                raise RuntimeError(
                    "live exchange evidence missing\n"
                    f"--- pinned Python ---\n{python_output}"
                    f"--- native Prns ---\n{rust_output}"
                )
            print(PYTHON_SUCCESS)
            print(RUST_SUCCESS)
            print("PASS: pinned Python LXMF and native Prns exchanged proven direct packets")
            return 0
        finally:
            stop(rust_process)
            stop(python_process)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
