import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO


ROOT = Path(__file__).resolve().parents[3]
STOCK_PEER = ROOT / "validation/interop/peers/rns_announce_app_data_peer.py"
NAPI_PEER = ROOT / "prns-napi/tests/interop/announce_app_data_peer.mjs"
STOCK_READY = "ANNOUNCE_APP_DATA_PEER_UP"
STOCK_OK = "STOCK_ANNOUNCE_APP_DATA_OK received=1"
NAPI_OK = "NAPI_ANNOUNCE_APP_DATA_OK received=1"
STOCK_START_TIMEOUT_SECONDS = 10
INTEROP_TIMEOUT_SECONDS = 40
PEER_STOP_TIMEOUT_SECONDS = 5
SUCCESS_MESSAGE = (
    "PASS: stock RNS 1.4.2 and Prns each preserved exact opaque announce application bytes"
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


def run_build_command(command: list[str], failure: str) -> None:
    try:
        result = subprocess.run(
            command,
            cwd=ROOT / "prns-napi",
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


def build_napi(npm: str) -> None:
    run_build_command(
        [npm, "ci", "--ignore-scripts", "--no-audit", "--no-fund"],
        "napi dependency install failed",
    )
    run_build_command([npm, "run", "build:debug"], "napi addon build failed")


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
                f"{peer.name} exited with status {return_code} "
                f"before reporting {marker}"
            )
        time.sleep(0.1)
    raise InteropFailure(f"{peer.name} did not report {marker}")


def wait_for_completion(peers: list[tuple[Peer, str]], timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        pending = [
            (peer, marker)
            for peer, marker in peers
            if marker not in read_log(peer)
        ]
        if not pending:
            return
        for peer, marker in pending:
            return_code = peer.process.poll()
            if return_code is not None:
                raise InteropFailure(
                    f"{peer.name} exited with status {return_code} "
                    f"before reporting {marker}"
                )
        time.sleep(0.25)
    missing = ", ".join(
        marker for peer, marker in peers if marker not in read_log(peer)
    )
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


def run_interop(work: Path, python: Path, node: str) -> None:
    stock_log = work / "stock.log"
    napi_log = work / "napi.log"
    stock = None
    napi = None
    try:
        port = allocate_port()
        stock_environment = os.environ.copy()
        stock_environment.update(
            {
                "PRNS_ANNOUNCE_APP_DATA_PORT": str(port),
                "PRNS_ANNOUNCE_APP_DATA_CONFIG_DIR": str(work / "stock-rns"),
            }
        )
        stock = start_peer(
            "stock announce application data peer",
            [str(python), str(STOCK_PEER)],
            stock_environment,
            stock_log,
        )
        wait_for_marker(stock, STOCK_READY, STOCK_START_TIMEOUT_SECONDS)

        napi_environment = os.environ.copy()
        napi_environment["PRNS_TCP_TARGET"] = f"127.0.0.1:{port}"
        napi = start_peer(
            "Prns NAPI announce application data peer",
            [node, str(NAPI_PEER)],
            napi_environment,
            napi_log,
        )
        wait_for_completion(
            [(stock, STOCK_OK), (napi, NAPI_OK)], INTEROP_TIMEOUT_SECONDS
        )
    finally:
        stop_peer(napi)
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
    node = shutil.which("node")
    if node is None:
        print("FAIL: node is required", file=sys.stderr)
        return 1
    if not os.environ.get("PRNS_NAPI_PREBUILT"):
        npm = shutil.which("npm")
        if npm is None:
            print("FAIL: npm is required to build the NAPI addon", file=sys.stderr)
            return 1
        try:
            build_napi(npm)
        except InteropFailure as error:
            print(f"FAIL: {error}", file=sys.stderr)
            return 1

    with tempfile.TemporaryDirectory() as work_directory:
        work = Path(work_directory)
        stock_log = work / "stock.log"
        napi_log = work / "napi.log"
        try:
            run_interop(work, python, node)
        except (InteropFailure, OSError) as error:
            print(f"FAIL: {error}", file=sys.stderr)
            print_log("stock RNS", stock_log)
            print_log("Prns NAPI", napi_log)
            return 1

    print(SUCCESS_MESSAGE)
    return 0


if __name__ == "__main__":
    sys.exit(main())
