#!/usr/bin/env python3
"""Run the pinned LXMF peer with temporary state for an explicit device target."""

from __future__ import annotations

import argparse
import ipaddress
import os
import pathlib
import socket
import subprocess
import sys
import tempfile

from tcp_fixture import (
    DEFAULT_LISTEN_IP,
    LISTEN_IP_ENV,
    WILDCARD_OPT_IN_ENV,
    tcp_target,
    validated_ip,
    validated_port,
)


ROOT = pathlib.Path(__file__).resolve().parents[5]
PEER = pathlib.Path(__file__).with_name("live_tcp_lxmf_peer.py")
DEFAULT_VENV = ROOT / "applications" / "target" / "interop" / "lxmf-1.1.0"


def arguments(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--listen-ip",
        default=DEFAULT_LISTEN_IP,
        help="explicit server bind address; defaults to loopback",
    )
    parser.add_argument(
        "--advertise-ip",
        help="concrete address the phone should use; required for a wildcard bind",
    )
    parser.add_argument("--port", type=int, help="fixed TCP port; defaults to an available port")
    parser.add_argument(
        "--allow-wildcard-bind",
        action="store_true",
        help="explicitly permit 0.0.0.0 or ::; exposes the fixture on reachable networks",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=int,
        default=300,
        help="exchange deadline from 1 through 3600 seconds",
    )
    parsed = parser.parse_args(argv)

    try:
        listen_address = validated_ip(parsed.listen_ip, label="--listen-ip")
        if listen_address.is_unspecified and not parsed.allow_wildcard_bind:
            parser.error("a wildcard --listen-ip requires --allow-wildcard-bind")
        if listen_address.is_unspecified and parsed.advertise_ip is None:
            parser.error("a wildcard --listen-ip requires a concrete --advertise-ip")
        advertise_address = validated_ip(
            parsed.advertise_ip or str(listen_address), label="--advertise-ip"
        )
        if advertise_address.is_unspecified:
            parser.error("--advertise-ip must not be a wildcard address")
        if advertise_address.version != listen_address.version:
            parser.error("listen and advertise addresses must use the same IP family")
        if parsed.port is not None:
            validated_port(parsed.port)
        if not 1 <= parsed.timeout_seconds <= 3600:
            parser.error("--timeout-seconds must be from 1 through 3600")
    except ValueError as error:
        parser.error(str(error))

    parsed.listen_address = listen_address
    parsed.advertise_address = advertise_address
    return parsed


def available_port(address: ipaddress.IPv4Address | ipaddress.IPv6Address) -> int:
    family = socket.AF_INET6 if address.version == 6 else socket.AF_INET
    with socket.socket(family, socket.SOCK_STREAM) as listener:
        listener.bind((str(address), 0))
        return int(listener.getsockname()[1])


def pinned_python() -> pathlib.Path:
    environment_path = os.environ.get("LXMF_VENV")
    venv = pathlib.Path(environment_path) if environment_path else DEFAULT_VENV
    executable = venv / "bin" / "python"
    if not executable.is_file():
        raise RuntimeError(
            f"pinned LXMF Python is missing at {executable}; run applications/services/lxmf/scripts/verify.sh first"
        )
    return executable


def stop(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main(argv: list[str] | None = None) -> int:
    selected = arguments(argv)
    port = selected.port or available_port(selected.listen_address)
    target = tcp_target(str(selected.advertise_address), port)
    environment = os.environ.copy()
    environment[LISTEN_IP_ENV] = str(selected.listen_address)
    environment["PRNS_LXMF_TCP_PORT"] = str(port)
    environment["PRNS_LXMF_EXCHANGE_TIMEOUT_SECONDS"] = str(selected.timeout_seconds)
    if selected.listen_address.is_unspecified:
        environment[WILDCARD_OPT_IN_ENV] = "1"
    else:
        environment.pop(WILDCARD_OPT_IN_ENV, None)

    process: subprocess.Popen[bytes] | None = None
    with tempfile.TemporaryDirectory(prefix="prns-lxmf-physical-") as temporary:
        environment["PRNS_LXMF_CONFIG_DIR"] = str(pathlib.Path(temporary) / "rns")
        print(f"PRNS_LXMF_PHYSICAL_TARGET={target}", flush=True)
        if selected.listen_address.is_unspecified:
            print(
                "WARNING: the pinned LXMF fixture is listening on all addresses until this process exits",
                flush=True,
            )
        try:
            process = subprocess.Popen(
                [str(pinned_python()), str(PEER)],
                cwd=ROOT,
                env=environment,
            )
            return process.wait()
        finally:
            stop(process)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130) from None
    except (OSError, RuntimeError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error
