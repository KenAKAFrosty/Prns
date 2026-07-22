#!/usr/bin/env python3

import socket
import sys
import time
from pathlib import Path

from RNS.Interfaces.RNodeInterface import KISS


HOST = "127.0.0.1"
PORT = 7633
FREQUENCY = 868_000_000
BANDWIDTH = 125_000
TXPOWER = 7
SPREADING_FACTOR = 8
CODING_RATE = 5


def frame(command, payload):
    return bytes([KISS.FEND, command]) + KISS.escape(payload) + bytes([KISS.FEND])


def received_frames(connection):
    current = bytearray()
    escaped = False
    while True:
        data = connection.recv(4096)
        if not data:
            return
        for byte in data:
            if byte == KISS.FEND:
                if current:
                    yield current[0], bytes(current[1:])
                    current.clear()
                escaped = False
            elif escaped:
                current.append(KISS.FEND if byte == KISS.TFEND else KISS.FESC if byte == KISS.TFESC else byte)
                escaped = False
            elif byte == KISS.FESC:
                escaped = True
            else:
                current.append(byte)


def send_split(connection, encoded):
    for index in range(0, len(encoded), 2):
        connection.sendall(encoded[index : index + 2])
        time.sleep(0.002)


def wait_for_detect(connection):
    for command, payload in received_frames(connection):
        if command == KISS.CMD_DETECT:
            if payload != bytes([KISS.DETECT_REQ]):
                raise RuntimeError(f"unexpected detect payload: {payload.hex()}")
            return
    raise RuntimeError("Prnsd disconnected before detection")


def wait_for_disconnect(connection):
    connection.settimeout(8)
    while connection.recv(4096):
        pass


def expected_reports():
    return {
        KISS.CMD_FREQUENCY: FREQUENCY.to_bytes(4, "big"),
        KISS.CMD_BANDWIDTH: BANDWIDTH.to_bytes(4, "big"),
        KISS.CMD_TXPOWER: bytes([TXPOWER]),
        KISS.CMD_SF: bytes([SPREADING_FACTOR]),
        KISS.CMD_CR: bytes([CODING_RATE]),
        KISS.CMD_RADIO_STATE: bytes([KISS.RADIO_STATE_ON]),
    }


def collect_configuration(connection, expected):
    configured = set()
    for command, payload in received_frames(connection):
        if command in expected:
            if payload != expected[command]:
                raise RuntimeError(
                    f"command {command:#04x}: expected {expected[command].hex()}, got {payload.hex()}"
                )
            configured.add(command)
            if configured == set(expected):
                return
    raise RuntimeError("Prnsd disconnected before writing RNode configuration")


def prepare(config_directory):
    directory = Path(config_directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "config").write_text(
        "[reticulum]\n"
        "share_instance = No\n"
        "enable_transport = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 7\n"
        "[interfaces]\n"
        "[[TCP RNode]]\n"
        "type = RNodeInterface\n"
        "enabled = Yes\n"
        "port = tcp://127.0.0.1\n"
        f"frequency = {FREQUENCY}\n"
        f"bandwidth = {BANDWIDTH}\n"
        f"txpower = {TXPOWER}\n"
        f"spreadingfactor = {SPREADING_FACTOR}\n"
        f"codingrate = {CODING_RATE}\n",
        encoding="utf-8",
    )


def serve(ready_path):
    expected = expected_reports()
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((HOST, PORT))
        listener.listen(4)
        Path(ready_path).touch()

        connection, _ = listener.accept()
        with connection:
            connection.settimeout(12)
            wait_for_detect(connection)
        print("RNODE_DISCONNECT_REJECTED", flush=True)

        connection, _ = listener.accept()
        with connection:
            connection.settimeout(12)
            wait_for_detect(connection)
            send_split(connection, frame(0xEE, bytes([KISS.FEND, KISS.FESC])))
            send_split(connection, frame(KISS.CMD_DETECT, bytes([KISS.DETECT_RESP])))
            send_split(connection, frame(KISS.CMD_FW_VERSION, bytes([1, 80])))
            collect_configuration(connection, expected)
            invalid = dict(expected)
            invalid[KISS.CMD_SF] = bytes([SPREADING_FACTOR + 1])
            for command, payload in reversed(list(invalid.items())):
                send_split(connection, frame(command, payload))
            wait_for_disconnect(connection)
        print("RNODE_INVALID_REPORT_REJECTED", flush=True)

        connection, _ = listener.accept()
        with connection:
            connection.settimeout(12)
            wait_for_detect(connection)
            for command, payload in reversed(list(expected.items())):
                send_split(connection, frame(command, payload))
            send_split(connection, frame(KISS.CMD_DETECT, bytes([KISS.DETECT_RESP])))
            collect_configuration(connection, expected)
            wait_for_disconnect(connection)
        print("RNODE_OUT_OF_ORDER_REJECTED", flush=True)

        connection, _ = listener.accept()
        with connection:
            connection.settimeout(12)
            wait_for_detect(connection)
            send_split(connection, frame(0xEE, bytes([KISS.FEND, KISS.FESC])))
            send_split(connection, frame(KISS.CMD_DETECT, bytes([KISS.DETECT_RESP])))
            send_split(connection, frame(KISS.CMD_FW_VERSION, bytes([1, 80])))
            collect_configuration(connection, expected)
            for command, payload in reversed(list(expected.items())):
                send_split(connection, frame(command, payload))
            for command, payload in received_frames(connection):
                if command == KISS.CMD_DETECT and payload == bytes([KISS.DETECT_REQ]):
                    print("RNODE_TCP_DEVICE_OK hostile_cases=3 split_frames=1", flush=True)
                    return
    raise RuntimeError("Prnsd disconnected before valid RNode recovery")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "prepare":
        prepare(sys.argv[2])
    elif len(sys.argv) == 3 and sys.argv[1] == "serve":
        serve(sys.argv[2])
    else:
        raise SystemExit(f"usage: {sys.argv[0]} prepare CONFIG_DIR | serve READY_FILE")
