#!/usr/bin/env python3

import socket
import sys
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
    expected = {
        KISS.CMD_FREQUENCY: FREQUENCY.to_bytes(4, "big"),
        KISS.CMD_BANDWIDTH: BANDWIDTH.to_bytes(4, "big"),
        KISS.CMD_TXPOWER: bytes([TXPOWER]),
        KISS.CMD_SF: bytes([SPREADING_FACTOR]),
        KISS.CMD_CR: bytes([CODING_RATE]),
        KISS.CMD_RADIO_STATE: bytes([KISS.RADIO_STATE_ON]),
    }
    configured = set()
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((HOST, PORT))
        listener.listen(1)
        Path(ready_path).touch()
        connection, _ = listener.accept()
        with connection:
            connection.settimeout(12)
            for command, payload in received_frames(connection):
                if command == KISS.CMD_DETECT:
                    if payload != bytes([KISS.DETECT_REQ]):
                        raise RuntimeError(f"unexpected detect payload: {payload.hex()}")
                    connection.sendall(frame(KISS.CMD_DETECT, bytes([KISS.DETECT_RESP])))
                    if configured == set(expected):
                        print("RNODE_TCP_DEVICE_OK", flush=True)
                        return
                elif command == KISS.CMD_FW_VERSION:
                    connection.sendall(frame(KISS.CMD_FW_VERSION, bytes([1, 80])))
                elif command in expected:
                    if payload != expected[command]:
                        raise RuntimeError(
                            f"command {command:#04x}: expected {expected[command].hex()}, got {payload.hex()}"
                        )
                    connection.sendall(frame(command, payload))
                    configured.add(command)
    raise RuntimeError("Prnsd disconnected before completing RNode configuration")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "prepare":
        prepare(sys.argv[2])
    elif len(sys.argv) == 3 and sys.argv[1] == "serve":
        serve(sys.argv[2])
    else:
        raise SystemExit(f"usage: {sys.argv[0]} prepare CONFIG_DIR | serve READY_FILE")
