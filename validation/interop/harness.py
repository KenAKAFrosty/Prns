from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Mapping, Sequence

from validation.interop.peers.rns_protocol_evidence import (
    PROTOCOL_EVIDENCE_FINAL,
    PROTOCOL_EVIDENCE_READY,
    PROTOCOL_EVIDENCE_SCHEMA,
)


ROOT = Path(__file__).resolve().parents[2]
PEER_STOP_TIMEOUT_SECONDS = 5
COMMAND_OUTPUT_ENCODING = "utf-8"
PYTHON_IO_ENCODING = f"{COMMAND_OUTPUT_ENCODING}:strict"
WORKSPACE_CLEANUP_TIMEOUT_SECONDS = 10.0
WORKSPACE_CLEANUP_RETRY_SECONDS = 0.05


class FailureKind(Enum):
    MISSING_REFERENCE_INTERPRETER = "missing reference interpreter"
    MISSING_REFERENCE_UTILITY = "missing reference utility"
    COMMAND_FAILED = "command failed"
    COMMAND_OUTPUT_INVALID = "command output invalid"
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


@dataclass(frozen=True)
class CommandStreams:
    standard_output: str
    standard_error: str


class CommandOutputCapture(Enum):
    MERGED = "merged"
    SEPARATE = "separate"


class CommandStream(Enum):
    COMBINED = "combined output"
    STANDARD_OUTPUT = "standard output"
    STANDARD_ERROR = "standard error"
    PROCESS_LOG = "process log"


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
    configured["PYTHONIOENCODING"] = PYTHON_IO_ENCODING
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


def reference_utility(name: str, environment_name: str = "RPC_SMOKE_PYTHON") -> Path:
    python = reference_python(environment_name)
    executable = name + (".exe" if os.name == "nt" else "")
    candidate = python.parent / executable
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise InteropFailure(
            FailureKind.MISSING_REFERENCE_UTILITY,
            f"stock RNS utility {name} is unavailable at {candidate}",
        )
    return candidate


def run_checked(
    command: Sequence[str],
    failure: str,
    working_directory: Path = ROOT,
    command_environment: Mapping[str, str] | None = None,
) -> str:
    result = _run_command(
        command,
        failure,
        working_directory,
        command_environment,
        CommandOutputCapture.MERGED,
    )
    if result.returncode != 0:
        output = decode_command_diagnostic(result.stdout).rstrip()
        detail = f"{failure}\n{output}" if output else failure
        raise InteropFailure(FailureKind.COMMAND_FAILED, detail)
    return decode_command_output(result.stdout, CommandStream.COMBINED, failure)


def run_expect_status(
    command: Sequence[str],
    expected_status: int,
    failure: str,
    working_directory: Path = ROOT,
    command_environment: Mapping[str, str] | None = None,
) -> str:
    result = _run_command(
        command,
        failure,
        working_directory,
        command_environment,
        CommandOutputCapture.MERGED,
    )
    if result.returncode != expected_status:
        output = decode_command_diagnostic(result.stdout).rstrip()
        status_failure = f"{failure}: expected status {expected_status}, got {result.returncode}"
        detail = f"{status_failure}\n{output}" if output else status_failure
        raise InteropFailure(FailureKind.COMMAND_FAILED, detail)
    return decode_command_output(result.stdout, CommandStream.COMBINED, failure)


def run_expect_status_with_streams(
    command: Sequence[str],
    expected_status: int,
    failure: str,
    working_directory: Path = ROOT,
    command_environment: Mapping[str, str] | None = None,
) -> CommandStreams:
    result = _run_command(
        command,
        failure,
        working_directory,
        command_environment,
        CommandOutputCapture.SEPARATE,
    )
    standard_error = result.stderr or b""
    if result.returncode != expected_status:
        rendered = "\n".join(
            output
            for output in (
                decode_command_diagnostic(result.stdout).rstrip(),
                decode_command_diagnostic(standard_error).rstrip(),
            )
            if output
        )
        status_failure = f"{failure}: expected status {expected_status}, got {result.returncode}"
        detail = f"{status_failure}\n{rendered}" if rendered else status_failure
        raise InteropFailure(FailureKind.COMMAND_FAILED, detail)
    return CommandStreams(
        decode_command_output(result.stdout, CommandStream.STANDARD_OUTPUT, failure),
        decode_command_output(standard_error, CommandStream.STANDARD_ERROR, failure),
    )


def _run_command(
    command: Sequence[str],
    failure: str,
    working_directory: Path,
    command_environment: Mapping[str, str] | None,
    capture: CommandOutputCapture,
) -> subprocess.CompletedProcess[bytes]:
    standard_error = (
        subprocess.STDOUT
        if capture is CommandOutputCapture.MERGED
        else subprocess.PIPE
    )
    try:
        return subprocess.run(
            command,
            cwd=working_directory,
            stdout=subprocess.PIPE,
            stderr=standard_error,
            check=False,
            env=_command_environment(command_environment),
        )
    except OSError as error:
        raise InteropFailure(FailureKind.COMMAND_FAILED, f"{failure}: {error}") from error


def _command_environment(
    command_environment: Mapping[str, str] | None,
) -> dict[str, str]:
    configured = (
        os.environ.copy()
        if command_environment is None
        else dict(command_environment)
    )
    configured["PYTHONIOENCODING"] = PYTHON_IO_ENCODING
    return configured


def translate_newlines(decoded: str) -> str:
    """Apply the universal-newline translation `text=True` used to provide.

    Decoding bytes keeps the platform line ending, so a child that prints on Windows
    yields carriage-return line feeds where callers and expected values assume line
    feeds alone.
    """
    return decoded.replace("\r\n", "\n").replace("\r", "\n")


def decode_command_output(
    output: bytes,
    stream: CommandStream,
    failure: str,
) -> str:
    try:
        return translate_newlines(output.decode(COMMAND_OUTPUT_ENCODING))
    except UnicodeDecodeError as error:
        raise InteropFailure(
            FailureKind.COMMAND_OUTPUT_INVALID,
            f"{failure}: {stream.value} is not UTF-8 at byte {error.start}",
        ) from error


def decode_command_diagnostic(output: bytes) -> str:
    return translate_newlines(output.decode(COMMAND_OUTPUT_ENCODING, errors="replace"))


def cleanup_temporary_directory(
    temporary: tempfile.TemporaryDirectory,
) -> None:
    deadline = time.monotonic() + WORKSPACE_CLEANUP_TIMEOUT_SECONDS
    while True:
        try:
            temporary.cleanup()
            return
        except OSError:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise
            time.sleep(min(WORKSPACE_CLEANUP_RETRY_SECONDS, remaining))


def run_checked_bytes(
    command: Sequence[str],
    failure: str,
    standard_input: bytes | None = None,
    working_directory: Path = ROOT,
    command_environment: Mapping[str, str] | None = None,
) -> bytes:
    try:
        result = subprocess.run(
            command,
            cwd=working_directory,
            input=standard_input,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=_command_environment(command_environment),
        )
    except OSError as error:
        raise InteropFailure(FailureKind.COMMAND_FAILED, f"{failure}: {error}") from error
    if result.returncode != 0:
        output = decode_command_diagnostic(result.stderr).rstrip()
        detail = f"{failure}\n{output}" if output else failure
        raise InteropFailure(FailureKind.COMMAND_FAILED, detail)
    return result.stdout


def require_evidence(condition: bool, failure: str) -> None:
    if not condition:
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure)


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


def require_listening_destination(output: str, failure: str) -> str:
    match = re.search(r"^Listening on : <([0-9a-f]{32})>$", output, re.MULTILINE)
    if match is None:
        rendered = output.rstrip()
        detail = f"{failure}\n{rendered}" if rendered else failure
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, detail)
    return match.group(1)


def require_no_protocol_violations_output(output: str, peer_name: str) -> None:
    matches = re.findall(
        rf"^{re.escape(PROTOCOL_EVIDENCE_FINAL)}(.+)$",
        output,
        re.MULTILINE,
    )
    if not matches:
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer_name} did not provide final RNS protocol evidence",
        )
    try:
        snapshot = json.loads(matches[-1])
    except json.JSONDecodeError as error:
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer_name} returned malformed final RNS protocol evidence",
        ) from error
    if not isinstance(snapshot, dict):
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer_name} returned malformed final RNS protocol evidence: {snapshot!r}",
        )
    require_no_protocol_violations_snapshot(snapshot, peer_name)


def require_no_protocol_violations_snapshot(
    snapshot: Mapping[str, object],
    peer_name: str,
) -> None:
    interfaces = snapshot.get("interfaces")
    schema = snapshot.get("schema")
    if (
        type(schema) is not int
        or schema != PROTOCOL_EVIDENCE_SCHEMA
        or not isinstance(interfaces, list)
    ):
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer_name} returned malformed RNS protocol evidence: {snapshot!r}",
        )
    if not interfaces:
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer_name} reported no RNS interfaces",
        )
    violations = []
    for interface in interfaces:
        if not isinstance(interface, dict):
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                f"{peer_name} returned a malformed RNS interface row: {interface!r}",
            )
        count = interface.get("protocol_violations")
        if type(count) is not int:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                f"{peer_name} did not expose the RNS 1.5 protocol counter: {interface!r}",
            )
        if count != 0:
            violations.append(interface)
    if violations:
        raise InteropFailure(
            FailureKind.EVIDENCE_UNEXPECTED,
            f"{peer_name} recorded RNS protocol violations: {violations!r}",
        )
    print(f'RNS_PROTOCOL_CLEAN peer="{peer_name}" interfaces={len(interfaces)}')


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
        self._reference_rns_peers: list[Peer] = []

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

    def start_reference_rns(
        self,
        spec: PeerSpec,
        listen_port: PortLease | None = None,
    ) -> Peer:
        peer = self.start(spec, listen_port)
        self._reference_rns_peers.append(peer)
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
        return_code = self.wait_for_status(peer, timeout_seconds)
        if return_code != 0:
            raise InteropFailure(
                FailureKind.PEER_EXITED,
                f"{peer.spec.name} exited with status {return_code}",
            )

    def wait_for_status(self, peer: Peer, timeout_seconds: float) -> int:
        try:
            return peer.process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired as error:
            raise InteropFailure(
                FailureKind.PEER_EXIT_TIMEOUT,
                f"timed out waiting for {peer.spec.name} to exit",
            ) from error

    def require_running(self, peer: Peer, duration_seconds: float, failure: str) -> None:
        deadline = time.monotonic() + duration_seconds
        while time.monotonic() < deadline:
            return_code = peer.process.poll()
            if return_code is not None:
                raise InteropFailure(
                    FailureKind.PEER_EXITED,
                    f"{failure}: {peer.spec.name} exited with status {return_code}",
                )
            time.sleep(0.05)

    def require_no_protocol_violations(self, *peers: Peer) -> None:
        for peer in peers:
            snapshot = self._protocol_evidence(peer)
            require_no_protocol_violations_snapshot(snapshot, peer.spec.name)

    def _protocol_evidence(self, peer: Peer) -> dict[str, object]:
        final = self._final_protocol_evidence(peer)
        if final is not None:
            return final
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            log = self.read_log(peer)
            match = re.search(
                rf"^{re.escape(PROTOCOL_EVIDENCE_READY)}([0-9]+)$",
                log,
                re.MULTILINE,
            )
            if match is not None:
                try:
                    with socket.create_connection(
                        ("127.0.0.1", int(match.group(1))), timeout=1
                    ) as connection:
                        with connection.makefile("rb") as stream:
                            response = stream.readline()
                    decoded = json.loads(response)
                    if isinstance(decoded, dict):
                        return decoded
                except (OSError, json.JSONDecodeError):
                    final = self._final_protocol_evidence(peer)
                    if final is not None:
                        return final
            if peer.process.poll() is not None:
                final = self._final_protocol_evidence(peer)
                if final is not None:
                    return final
            time.sleep(0.05)
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            f"{peer.spec.name} did not provide RNS protocol evidence",
        )

    def _final_protocol_evidence(self, peer: Peer) -> dict[str, object] | None:
        matches = re.findall(
            rf"^{re.escape(PROTOCOL_EVIDENCE_FINAL)}(.+)$",
            self.read_log(peer),
            re.MULTILINE,
        )
        if not matches:
            return None
        try:
            decoded = json.loads(matches[-1])
        except json.JSONDecodeError:
            return None
        return decoded if isinstance(decoded, dict) else None

    def terminate(self, peer: Peer) -> int:
        if peer.process.poll() is not None:
            return peer.process.returncode
        try:
            peer.process.terminate()
        except ProcessLookupError:
            return peer.process.wait()
        try:
            return peer.process.wait(timeout=PEER_STOP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            try:
                peer.process.kill()
            except ProcessLookupError:
                pass
            return peer.process.wait()

    def stop(self, peer: Peer) -> None:
        try:
            self.terminate(peer)
        finally:
            peer.log_file.close()

    def print_logs(self) -> None:
        for peer in self._peers:
            contents = self.read_log(peer)
            if not contents:
                continue
            print(f"{peer.spec.name} log:", file=sys.stderr)
            print(contents, file=sys.stderr, end="" if contents.endswith("\n") else "\n")

    def _cleanup_workspace(self) -> None:
        cleanup_temporary_directory(self._temporary)

    def __enter__(self) -> InteropCase:
        return self

    def __exit__(self, kind, _value, _traceback) -> None:
        evidence_failure = None
        if kind is None:
            try:
                self.require_no_protocol_violations(*self._reference_rns_peers)
            except InteropFailure as error:
                evidence_failure = error
        for peer in reversed(self._peers):
            self.stop(peer)
        if kind is not None or evidence_failure is not None:
            self.print_logs()
        self._cleanup_workspace()
        if evidence_failure is not None:
            raise evidence_failure


def case_main(run: Callable[[], None], success_message: str) -> int:
    try:
        run()
    except (InteropFailure, OSError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(success_message)
    return 0
