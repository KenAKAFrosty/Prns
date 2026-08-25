import os
import pathlib
import sys
import tempfile
import threading
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


PAYLOAD_SIZE = 4096
RECEIVE_STREAM_ID = 7
SEND_STREAM_ID = 11
WRITE_BOUNDARIES = (33, 701, 5, 1531, 89)
READ_BOUNDARIES = (17, 509, 3, 2047, 61)
CANDIDATE_PAYLOAD = bytes((index * 29 + 7) % 256 for index in range(PAYLOAD_SIZE))
STOCK_PAYLOAD = bytes((index * 17 + 3) % 256 for index in range(PAYLOAD_SIZE))


def configuration(port):
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = No\n"
        "panic_on_interface_error = No\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[Buffer Stream TCP Server]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {port}\n"
    )


def main():
    port = int(os.environ["PRNS_BUFFER_STREAM_PORT"])
    config_dir = pathlib.Path(tempfile.mkdtemp(prefix="rns-buffer-stream-server-"))
    config_dir.joinpath("config").write_text(configuration(port), encoding="utf-8")
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "prns",
        "buffer-stream",
    )
    lock = threading.Lock()
    state = {
        "complete": False,
        "failure": None,
        "linked": False,
        "received": False,
        "sent": False,
    }

    def settle(name, value=True):
        with lock:
            state[name] = value
            if state["received"] and state["sent"] and not state["complete"]:
                state["complete"] = True
                print("STOCK_BUFFER_STREAM_OK received=4096 sent=4096 eof=1", flush=True)

    def fail(error):
        settle("failure", repr(error))

    def established(link):
        with lock:
            state["linked"] = True
        channel = link.get_channel()
        reader = RNS.Buffer.create_reader(RECEIVE_STREAM_ID, channel)
        writer = RNS.Buffer.create_writer(SEND_STREAM_ID, channel)

        def receive():
            try:
                received = bytearray()
                boundary = 0
                deadline = time.time() + 30
                while time.time() < deadline:
                    chunk = reader.read(READ_BOUNDARIES[boundary % len(READ_BOUNDARIES)])
                    if chunk is None:
                        time.sleep(0.01)
                        continue
                    if chunk == b"":
                        if bytes(received) != CANDIDATE_PAYLOAD:
                            raise RuntimeError(
                                f"unexpected Prns stream bytes={len(received)}"
                            )
                        settle("received")
                        return
                    received.extend(chunk)
                    boundary += 1
                raise RuntimeError(f"Prns stream did not reach EOF bytes={len(received)}")
            except Exception as error:
                fail(error)

        def send():
            try:
                offset = 0
                boundary = 0
                while offset < len(STOCK_PAYLOAD):
                    end = min(
                        offset + WRITE_BOUNDARIES[boundary % len(WRITE_BOUNDARIES)],
                        len(STOCK_PAYLOAD),
                    )
                    written = writer.write(STOCK_PAYLOAD[offset:end])
                    if written != end - offset:
                        raise RuntimeError(
                            f"stock stream accepted {written} of {end - offset} bytes"
                        )
                    writer.flush()
                    offset = end
                    boundary += 1
                writer.close()
                settle("sent")
            except Exception as error:
                fail(error)

        threading.Thread(target=receive, name="Buffer Receive", daemon=True).start()
        threading.Thread(target=send, name="Buffer Send", daemon=True).start()

    destination.set_link_established_callback(established)
    print(f"BUFFER_STREAM_SERVER_UP {destination.hash.hex()}", flush=True)
    deadline = time.time() + 40
    while time.time() < deadline:
        with lock:
            failure = state["failure"]
            complete = state["complete"]
            linked = state["linked"]
        if failure is not None:
            raise RuntimeError(failure)
        if complete:
            time.sleep(1)
            return 0
        if not linked:
            destination.announce()
        time.sleep(0.5)
    raise RuntimeError("buffer stream exchange timed out")


if __name__ == "__main__":
    sys.exit(main())
