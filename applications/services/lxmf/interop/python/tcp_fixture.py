"""Safe network configuration shared by the pinned LXMF TCP fixtures."""

from __future__ import annotations

import ipaddress
from collections.abc import Mapping


LISTEN_IP_ENV = "PRNS_LXMF_LISTEN_IP"
WILDCARD_OPT_IN_ENV = "PRNS_LXMF_ALLOW_WILDCARD_BIND"
DEFAULT_LISTEN_IP = "127.0.0.1"


def validated_ip(value: str, *, label: str) -> ipaddress.IPv4Address | ipaddress.IPv6Address:
    if not value or value != value.strip():
        raise ValueError(f"{label} must be one explicit IP address without surrounding whitespace")
    try:
        return ipaddress.ip_address(value)
    except ValueError as error:
        raise ValueError(f"{label} must be one explicit IPv4 or IPv6 address") from error


def environment_listen_ip(environment: Mapping[str, str]) -> str:
    address = validated_ip(environment.get(LISTEN_IP_ENV, DEFAULT_LISTEN_IP), label=LISTEN_IP_ENV)
    if address.is_unspecified and environment.get(WILDCARD_OPT_IN_ENV) != "1":
        raise ValueError(
            f"{LISTEN_IP_ENV}={address} requires explicit {WILDCARD_OPT_IN_ENV}=1"
        )
    return str(address)


def validated_port(port: int) -> int:
    if not 1 <= port <= 65535:
        raise ValueError("TCP fixture port must be from 1 through 65535")
    return port


def server_configuration(port: int, listen_ip: str) -> str:
    address = validated_ip(listen_ip, label="listen IP")
    port = validated_port(port)
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[LXMF TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        f"listen_ip = {address}\n"
        f"listen_port = {port}\n"
    )


def tcp_target(advertise_ip: str, port: int) -> str:
    address = validated_ip(advertise_ip, label="advertise IP")
    if address.is_unspecified:
        raise ValueError("advertise IP must be a concrete address, not a wildcard")
    port = validated_port(port)
    if address.version == 6:
        return f"[{address}]:{port}"
    return f"{address}:{port}"
