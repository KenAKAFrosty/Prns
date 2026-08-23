import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO


ROOT = Path(__file__).resolve().parents[3]
INTEGRATION_MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_PEER = ROOT / "validation/interop/peers/rns_plain_group_peer.py"
CANDIDATE_EXAMPLE = "plain_group_interop_peer"
STOCK_READY = "STOCK_PLAIN_GROUP_PEER_UP"
CANDIDATE_READY = "PRNS_PLAIN_GROUP_PEER_UP"
STOCK_OK = "STOCK_PLAIN_GROUP_OK received_plain=1 received_group=1"
CANDIDATE_OK = "PRNS_PLAIN_GROUP_OK received_plain=1 received_group=1"
START_TIMEOUT_SECONDS = 10
INTEROP_TIMEOUT_SECONDS = 45
PEER_STOP_TIMEOUT_SECONDS = 5
SUCCESS_MESSAGE = (
    "PASS: stock RNS 1.4.2 and Prns exchanged exact PLAIN and GROUP payloads in both directions"
)


class InteropFailure(RuntimeError):
    pass


@dataclass
class Peer:
    name: str
    process: subprocess.Popen[bytes]
    log_path: Path
    log_file: BinaryIO


def reference_python() -> Path:
    configured = os.environ.get("SMOKE_PYTHON")
    if configured:
        return Path(configured)
    executable = Path("Scripts/python.exe") if os.name == "nt" else Path("bin/python")
    return ROOT / "validation/.venv/rns-1.4.2" / executable


def allocate_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def run_command(command: list[str], failure: str) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
    except OSError as error:
        raise InteropFailure(f"{failure}: {error}") from error
    if result.returncode != 0:
        output = result.stdout.rstrip()
        detail = f"\n{output}" if output else ""
        raise InteropFailure(f"{failure}{detail}")
    return result.stdout


def build_candidate() -> Path:
    run_command(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(INTEGRATION_MANIFEST),
            "--example",
            CANDIDATE_EXAMPLE,
            "--locked",
        ],
        "Prns PLAIN/GROUP candidate build failed",
    )
    metadata = json.loads(
        run_command(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(INTEGRATION_MANIFEST),
                "--no-deps",
                "--format-version",
                "1",
            ],
            "could not locate the Prns integration target directory",
        )
    )
    executable = CANDIDATE_EXAMPLE + (".exe" if os.name == "nt" else "")
    return Path(metadata["target_directory"]) / "debug" / "examples" / executable


def start_peer(
    name: str,
    command: list[str],
    environment: dict[str, str],
    log_path: Path,
) -> Peer:
    log_file = log_path.open("wb")
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            env=environment,
            stdout=log_file,
            stderr=subprocess.STDOUT,
        )
    except OSError as error:
        log_file.close()
        raise InteropFailure(f"could not start {name}: {error}") from error
    return Peer(name, process, log_path, log_file)


def read_log(peer: Peer) -> str:
    try:
        return peer.log_path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def wait_for_marker(peer: Peer, marker: str, timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if marker in read_log(peer):
            return
        return_code = peer.process.poll()
        if return_code is not None:
            raise InteropFailure(
                f"{peer.name} exited with status {return_code} before reporting {marker}"
            )
        time.sleep(0.1)
    raise InteropFailure(f"{peer.name} did not report {marker}")


def wait_for_completion(peers: list[tuple[Peer, str]], timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        pending = [(peer, marker) for peer, marker in peers if marker not in read_log(peer)]
        if not pending:
            return
        for peer, marker in pending:
            return_code = peer.process.poll()
            if return_code is not None:
                raise InteropFailure(
                    f"{peer.name} exited with status {return_code} before reporting {marker}"
                )
        time.sleep(0.25)
    missing = ", ".join(marker for peer, marker in peers if marker not in read_log(peer))
    raise InteropFailure(f"bidirectional evidence timed out waiting for {missing}")


def stop_peer(peer: Peer | None) -> None:
    if peer is None:
        return
    try:
        if peer.process.poll() is None:
            try:
                peer.process.terminate()
            except ProcessLookupError:
                pass
            try:
                peer.process.wait(timeout=PEER_STOP_TIMEOUT_SECONDS)
            except subprocess.TimeoutExpired:
                try:
                    peer.process.kill()
                except ProcessLookupError:
                    pass
                peer.process.wait()
    finally:
        peer.log_file.close()


def run_interop(work: Path, python: Path, candidate: Path) -> None:
    stock = None
    prns = None
    try:
        port = allocate_port()
        stock_environment = os.environ.copy()
        stock_environment.update(
            {
                "PRNS_PLAIN_GROUP_PORT": str(port),
                "PRNS_PLAIN_GROUP_CONFIG_DIR": str(work / "stock-rns"),
            }
        )
        stock = start_peer(
            "stock RNS PLAIN/GROUP peer",
            [str(python), str(STOCK_PEER)],
            stock_environment,
            work / "stock.log",
        )
        wait_for_marker(stock, STOCK_READY, START_TIMEOUT_SECONDS)

        candidate_environment = os.environ.copy()
        candidate_environment["PRNS_PLAIN_GROUP_TARGET"] = f"127.0.0.1:{port}"
        prns = start_peer(
            "Prns PLAIN/GROUP peer",
            [str(candidate)],
            candidate_environment,
            work / "prns.log",
        )
        wait_for_marker(prns, CANDIDATE_READY, START_TIMEOUT_SECONDS)
        wait_for_completion(
            [(stock, STOCK_OK), (prns, CANDIDATE_OK)], INTEROP_TIMEOUT_SECONDS
        )
    finally:
        stop_peer(prns)
        stop_peer(stock)


def print_log(label: str, path: Path) -> None:
    if not path.exists():
        return
    contents = path.read_text(encoding="utf-8", errors="replace")
    if not contents:
        return
    print(f"{label} log:", file=sys.stderr)
    print(contents, file=sys.stderr, end="" if contents.endswith("\n") else "\n")


def main() -> int:
    python = reference_python()
    if not python.is_file() or not os.access(python, os.X_OK):
        print(f"FAIL: reference venv python not found at {python}", file=sys.stderr)
        return 1
    try:
        candidate = build_candidate()
    except InteropFailure as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory() as work_directory:
        work = Path(work_directory)
        try:
            run_interop(work, python, candidate)
        except (InteropFailure, OSError) as error:
            print(f"FAIL: {error}", file=sys.stderr)
            print_log("stock RNS", work / "stock.log")
            print_log("Prns", work / "prns.log")
            return 1

    print(SUCCESS_MESSAGE)
    return 0


if __name__ == "__main__":
    sys.exit(main())
