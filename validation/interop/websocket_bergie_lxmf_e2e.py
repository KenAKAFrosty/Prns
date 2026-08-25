from __future__ import annotations

import json
import os
import socket
import subprocess
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path

from validation.interop.harness import (
    CommandStream,
    cleanup_temporary_directory,
    decode_command_diagnostic,
    decode_command_output,
    environment,
)


ROOT = Path(__file__).resolve().parents[2]
PRNSD_MANIFEST = ROOT / "prnsd" / "Cargo.toml"
PRNSD_TARGET = ROOT / "prnsd" / "target"
PRNSD_BINARY = PRNSD_TARGET / "debug" / (
    "prnsd.exe" if os.name == "nt" else "prnsd"
)


def build_prnsd() -> None:
    command = [
        "cargo",
        "build",
        "--quiet",
        "--manifest-path",
        str(PRNSD_MANIFEST),
        "--target-dir",
        str(PRNSD_TARGET),
        "--locked",
        "--no-default-features",
        "--features",
        "tokio-cloud-host,observability",
        "--bin",
        "prnsd",
    ]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        check=False,
        capture_output=True,
        timeout=300,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            json.dumps(
                {
                    "command": command,
                    "cwd": str(ROOT),
                    "exit_code": completed.returncode,
                    "stdout": decode_command_diagnostic(completed.stdout),
                    "stderr": decode_command_diagnostic(completed.stderr),
                },
                sort_keys=True,
            )
        )


def available_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class LoggedProcess:
    def __init__(self, process: subprocess.Popen, log_path: Path):
        self.process = process
        self.log_path = log_path

    @classmethod
    def start(
        cls, command: list[str], cwd: Path, log_path: Path
    ) -> LoggedProcess:
        with log_path.open("wb") as log:
            process = subprocess.Popen(
                command,
                cwd=cwd,
                env=environment({"PYTHONDONTWRITEBYTECODE": "1"}),
                stdout=log,
                stderr=subprocess.STDOUT,
            )
        return cls(process, log_path)

    def __enter__(self) -> LoggedProcess:
        return self

    def __exit__(self, exception_type, exception, traceback) -> None:
        self.stop()

    def output(self) -> str:
        if not self.log_path.exists():
            return ""
        return decode_command_output(
            self.log_path.read_bytes(),
            CommandStream.PROCESS_LOG,
            f"{self.log_path} emitted invalid output",
        )

    def wait_for(self, expected: str, timeout_seconds: int) -> str:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            output = self.output()
            if expected in output:
                return output
            exit_code = self.process.poll()
            if exit_code is not None:
                raise RuntimeError(
                    f"process exited with {exit_code} before {expected!r}:\n{output}"
                )
            time.sleep(0.05)
        raise RuntimeError(
            f"timed out waiting for {expected!r}:\n{self.output()}"
        )

    def wait(self, timeout_seconds: int) -> str:
        try:
            exit_code = self.process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(
                f"process timed out after {timeout_seconds} seconds:\n{self.output()}"
            ) from error
        output = self.output()
        if exit_code != 0:
            raise RuntimeError(f"process exited with {exit_code}:\n{output}")
        return output

    def stop(self) -> None:
        if self.process.poll() is not None:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)


def read_log(log_path: Path) -> str:
    if not log_path.exists():
        return ""
    return decode_command_output(
        log_path.read_bytes(),
        CommandStream.PROCESS_LOG,
        f"{log_path} emitted invalid output",
    )


def marker_lines(output: str, prefix: str) -> list[str]:
    return [line for line in output.splitlines() if line.startswith(prefix)]


def destination_from(output: str, prefix: str) -> str:
    matching = marker_lines(output, prefix)
    if len(matching) != 1:
        raise RuntimeError(f"expected one {prefix!r} line, received {matching!r}")
    destination = matching[0].removeprefix(prefix)
    try:
        decoded = bytes.fromhex(destination)
    except ValueError as error:
        raise RuntimeError(f"invalid destination hash {destination!r}") from error
    if len(decoded) != 16:
        raise RuntimeError(f"invalid destination hash {destination!r}")
    return destination


def exercise(repository: Path, echo_adapter: Path, sender_adapter: Path) -> dict:
    build_prnsd()

    temporary = tempfile.TemporaryDirectory(prefix="prns-bergie-lxmf-")
    try:
        run_root = Path(temporary.name)
        config_root = run_root / "prnsd"
        config_root.mkdir()
        port = available_tcp_port()
        target = f"ws://127.0.0.1:{port}/prns"
        ready_signal = run_root / "sender-ready"
        daemon_log = run_root / "prnsd.log"
        echo_log = run_root / "echo.log"
        sender_log = run_root / "sender.log"
        config_root.joinpath("config").write_text(
            "[reticulum]\n"
            "enable_transport = Yes\n"
            "share_instance = No\n"
            "[logging]\n"
            "loglevel = 4\n"
            "logtimestamps = No\n"
            "[interfaces]\n"
            "[[Bergie WebSocket]]\n"
            "type = PrnsWebSocketServer\n"
            "interface_enabled = Yes\n"
            "listen_ip = 127.0.0.1\n"
            f"listen_port = {port}\n"
            "framing = auto\n",
            encoding="utf-8",
        )

        try:
            with ExitStack() as processes:
                daemon = processes.enter_context(
                    LoggedProcess.start(
                        [
                            str(PRNSD_BINARY),
                            "run",
                            "--log-format",
                            "json",
                            "--config",
                            str(config_root),
                        ],
                        ROOT,
                        daemon_log,
                    )
                )
                daemon_output = daemon.wait_for('"event":"daemon_ready"', 10)
                if '"medium":"prns_websocket_server"' not in daemon_output:
                    raise RuntimeError(
                        "prnsd reached readiness without its WebSocket server:\n"
                        f"{daemon_output}"
                    )

                echo = processes.enter_context(
                    LoggedProcess.start(
                        [
                            "node",
                            str(echo_adapter),
                            str(repository),
                            target,
                            str(ready_signal),
                        ],
                        repository,
                        echo_log,
                    )
                )
                echo_startup = echo.wait_for("ECHO_WAITING_FOR_SENDER", 10)
                echo_destination = destination_from(
                    echo_startup, "ECHO_DESTINATION "
                )

                sender = processes.enter_context(
                    LoggedProcess.start(
                        [
                            "node",
                            str(sender_adapter),
                            str(repository),
                            target,
                            echo_destination,
                            str(ready_signal),
                        ],
                        repository,
                        sender_log,
                    )
                )
                sender_output = sender.wait(170)
                echo_output = echo.wait(10)

                sender_destination = destination_from(
                    sender_output, "SENDER_DESTINATION "
                )
                expected_echo = [
                    f"ECHO_DESTINATION {echo_destination}",
                    "ECHO_WAITING_FOR_SENDER",
                    "ECHO_ANNOUNCED",
                    "ECHO_RECEIVED Hello through real prnsd",
                    "ECHO_REPLIED Echo: Hello through real prnsd",
                ]
                expected_sender = [
                    f"SENDER_DESTINATION {sender_destination}",
                    "SENDER_ANNOUNCED",
                    f"SENDER_LEARNED {echo_destination}",
                    "SENDER_SENT Hello through real prnsd",
                    "SENDER_RECEIVED Echo: Hello through real prnsd",
                ]
                actual_echo = marker_lines(echo_output, "ECHO_")
                actual_sender = marker_lines(sender_output, "SENDER_")
                if actual_echo != expected_echo:
                    raise RuntimeError(
                        "Bergie echo transcript changed: "
                        f"expected {expected_echo!r}, received {actual_echo!r}"
                    )
                if actual_sender != expected_sender:
                    raise RuntimeError(
                        "Bergie sender transcript changed: "
                        f"expected {expected_sender!r}, received {actual_sender!r}"
                    )
        except Exception as error:
            evidence = {
                "prnsd": read_log(daemon_log),
                "echo": read_log(echo_log),
                "sender": read_log(sender_log),
            }
            raise RuntimeError(
                f"Bergie LXMF application interoperability failed: {error}\n"
                f"{json.dumps(evidence, sort_keys=True)}"
            ) from error
    finally:
        cleanup_temporary_directory(temporary)

    return {
        "kind": "lxmf_application_e2e",
        "topology": {
            "sender": "bergie_raw",
            "relay": "prnsd_auto",
            "echo": "bergie_kiss",
        },
        "announce_forwarded": True,
        "request_delivered": True,
        "reply_delivered": True,
    }
