from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Mapping, Sequence


ROOT = Path(__file__).resolve().parents[2]
PEER_STOP_TIMEOUT_SECONDS = 5


class FailureKind(Enum):
    MISSING_REFERENCE_INTERPRETER = "missing reference interpreter"
    COMMAND_FAILED = "command failed"
    EVIDENCE_MISSING = "evidence missing"
    EVIDENCE_UNEXPECTED = "evidence unexpected"
    PEER_START_FAILED = "peer start failed"
    PEER_EXITED = "peer exited"
    PEER_EXIT_TIMEOUT = "peer exit timeout"
    PATH_TIMEOUT = "path timeout"
    LISTENER_TIMEOUT = "listener timeout"
    MARKER_TIMEOUT = "marker timeout"


class InteropFailure(RuntimeError):
    def __init__(self, kind: FailureKind, detail: str):
        self.kind = kind
        self.detail = detail
        super().__init__(f"{kind.value}: {detail}")


@dataclass(frozen=True)
class PeerSpec:
    name: str
    command: tuple[str, ...]
    environment: Mapping[str, str]


@dataclass
class Peer:
    spec: PeerSpec
    process: subprocess.Popen[bytes]
    log_path: Path
    log_file: object


class PortLease:
    def __init__(self):
        self._listener = socket.socket()
        self._listener.bind(("127.0.0.1", 0))
        self.port = self._listener.getsockname()[1]

    def release(self) -> None:
        if self._listener is None:
            return
        self._listener.close()
        self._listener = None

    def __enter__(self) -> PortLease:
        return self

    def __exit__(self, _kind, _value, _traceback) -> None:
        self.release()


def environment(
    values: Mapping[str, object],
    without: Sequence[str] = (),
) -> dict[str, str]:
    configured = os.environ.copy()
    for name in without:
        configured.pop(name, None)
    configured.update({key: str(value) for key, value in values.items()})
    return configured


def reference_python(environment_name: str = "SMOKE_PYTHON") -> Path:
    configured = os.environ.get(environment_name)
    if configured is None:
        raise InteropFailure(
            FailureKind.MISSING_REFERENCE_INTERPRETER,
            f"{environment_name} is unset; launch this case through validation/run.py",
        )
    candidate = Path(configured)
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise InteropFailure(
            FailureKind.MISSING_REFERENCE_INTERPRETER,
            f"{environment_name} does not name an executable: {candidate}",
        )
    return candidate


def run_checked(
    command: Sequence[str],
    failure: str,
    working_directory: Path = ROOT,
    command_environment: Mapping[str, str] | None = None,
) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=working_directory,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
            env=command_environment,
        )
    except OSError as error:
        raise InteropFailure(FailureKind.COMMAND_FAILED, f"{failure}: {error}") from error
    if result.returncode != 0:
        output = result.stdout.rstrip()
        detail = f"{failure}\n{output}" if output else failure
        raise InteropFailure(FailureKind.COMMAND_FAILED, detail)
    return result.stdout


def require_output_marker(output: str, marker: str, failure: str) -> None:
    if marker in output:
        return
    rendered = output.rstrip()
    detail = f"{failure}\n{rendered}" if rendered else failure
    raise InteropFailure(FailureKind.EVIDENCE_MISSING, detail)


def forbid_output_marker(output: str, marker: str, failure: str) -> None:
    if marker not in output:
        return
    rendered = output.rstrip()
    detail = f"{failure}\n{rendered}" if rendered else failure
    raise InteropFailure(FailureKind.EVIDENCE_UNEXPECTED, detail)


def require_hex_output(output: str, byte_length: int, failure: str) -> str:
    rendered = output.strip()
    try:
        decoded = bytes.fromhex(rendered)
    except ValueError as error:
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure) from error
    if len(decoded) != byte_length:
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure)
    return rendered


def _cargo_artifact(
    manifest: Path,
    selection: Sequence[str],
    artifact_path: Sequence[str],
    artifact_name: str,
) -> Path:
    run_checked(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(manifest),
            *selection,
            "--locked",
        ],
        f"Cargo artifact {artifact_name} did not build",
    )
    metadata = json.loads(
        run_checked(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(manifest),
                "--no-deps",
                "--format-version",
                "1",
            ],
            f"Cargo metadata did not locate {artifact_name}",
        )
    )
    executable = artifact_name + (".exe" if os.name == "nt" else "")
    return Path(metadata["target_directory"]).joinpath("debug", *artifact_path, executable)


def cargo_binary(manifest: Path, binary: str) -> Path:
    return _cargo_artifact(manifest, ("--bin", binary), (), binary)


def cargo_example(manifest: Path, example: str) -> Path:
    return _cargo_artifact(manifest, ("--example", example), ("examples",), example)


def candidate_peer() -> Path:
    return cargo_example(
        ROOT / "validation/integration/Cargo.toml",
        "rns_interop_peer",
    )


class InteropCase:
    def __init__(self):
        self._temporary = tempfile.TemporaryDirectory()
        self.work = Path(self._temporary.name)
        self._peers: list[Peer] = []

    def start(self, spec: PeerSpec, listen_port: PortLease | None = None) -> Peer:
        if listen_port is not None:
            listen_port.release()
        log_name = "".join(
            character if character.isalnum() or character in "-." else "-"
            for character in spec.name
        )
        log_path = self.work / f"{len(self._peers):02d}-{log_name}.log"
        log_file = log_path.open("wb", buffering=0)
        try:
            process = subprocess.Popen(
                spec.command,
                cwd=ROOT,
                env=spec.environment,
                stdout=log_file,
                stderr=subprocess.STDOUT,
            )
        except OSError as error:
            log_file.close()
            raise InteropFailure(
                FailureKind.PEER_START_FAILED,
                f"could not start {spec.name}: {error}",
            ) from error
        peer = Peer(spec, process, log_path, log_file)
        self._peers.append(peer)
        return peer

    def read_log(self, peer: Peer) -> str:
        try:
            return peer.log_path.read_text(encoding="utf-8", errors="replace")
        except FileNotFoundError:
            return ""

    def wait_for(self, peer: Peer, marker: str, timeout_seconds: float) -> None:
        self.wait_for_all([(peer, marker)], timeout_seconds)

    def wait_for_all(
        self,
        evidence: Sequence[tuple[Peer, str]],
        timeout_seconds: float,
        required_peers: Sequence[Peer] = (),
    ) -> None:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            pending = [
                (peer, marker)
                for peer, marker in evidence
                if marker not in self.read_log(peer)
            ]
            if not pending:
                return
            monitored = [(peer, marker) for peer, marker in pending]
            monitored.extend((peer, "required operation completed") for peer in required_peers)
            for peer, marker in monitored:
                return_code = peer.process.poll()
                if return_code is not None:
                    raise InteropFailure(
                        FailureKind.PEER_EXITED,
                        f"{peer.spec.name} exited with status {return_code} before {marker}",
                    )
            time.sleep(0.1)
        missing = ", ".join(marker for peer, marker in evidence if marker not in self.read_log(peer))
        raise InteropFailure(FailureKind.MARKER_TIMEOUT, f"timed out waiting for {missing}")

    def wait_for_listener(
        self,
        peer: Peer,
        host: str,
        port: int,
        timeout_seconds: float,
    ) -> None:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            return_code = peer.process.poll()
            if return_code is not None:
                raise InteropFailure(
                    FailureKind.PEER_EXITED,
                    f"{peer.spec.name} exited with status {return_code} before {host}:{port} listened",
                )
            try:
                connection = socket.create_connection((host, port), timeout=0.1)
            except OSError:
                time.sleep(0.1)
                continue
            connection.close()
            return
        raise InteropFailure(
            FailureKind.LISTENER_TIMEOUT,
            f"timed out waiting for {peer.spec.name} at {host}:{port}",
        )

    def wait_for_path(self, peer: Peer, path: Path, timeout_seconds: float) -> None:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            if path.exists():
                return
            return_code = peer.process.poll()
            if return_code is not None:
                raise InteropFailure(
                    FailureKind.PEER_EXITED,
                    f"{peer.spec.name} exited with status {return_code} before creating {path}",
                )
            time.sleep(0.1)
        raise InteropFailure(
            FailureKind.PATH_TIMEOUT,
            f"timed out waiting for {peer.spec.name} to create {path}",
        )

    def wait_for_exit(self, peer: Peer, timeout_seconds: float) -> None:
        try:
            return_code = peer.process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired as error:
            raise InteropFailure(
                FailureKind.PEER_EXIT_TIMEOUT,
                f"timed out waiting for {peer.spec.name} to exit",
            ) from error
        if return_code != 0:
            raise InteropFailure(
                FailureKind.PEER_EXITED,
                f"{peer.spec.name} exited with status {return_code}",
            )

    def stop(self, peer: Peer) -> None:
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

    def print_logs(self) -> None:
        for peer in self._peers:
            contents = self.read_log(peer)
            if not contents:
                continue
            print(f"{peer.spec.name} log:", file=sys.stderr)
            print(contents, file=sys.stderr, end="" if contents.endswith("\n") else "\n")

    def __enter__(self) -> InteropCase:
        return self

    def __exit__(self, kind, _value, _traceback) -> None:
        for peer in reversed(self._peers):
            self.stop(peer)
        if kind is not None:
            self.print_logs()
        self._temporary.cleanup()


def case_main(run: Callable[[], None], success_message: str) -> int:
    try:
        run()
    except (InteropFailure, OSError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(success_message)
    return 0
