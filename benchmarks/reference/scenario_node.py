#!/usr/bin/env python3
"""RNS 1.3.1's participation binary for live scenarios — the same contract as
`scenario_node`: `scenario_node.py <manifest.json> <role> <addr> [duration-ms]`,
READY/RESULT lines on stdout. The responder serves a ProveAll destination over a real
TCPServerInterface; the initiator connects with a TCPClientInterface, establishes one
link, and pumps windowed sends, measuring from the reference's own packet receipts."""

import json
import os
import socket
import sys
import tempfile
import threading
import time

import RNS

ANNOUNCE_EVERY = 0.5
DRAIN_GRACE = 5.0


def free_port():
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def start_reticulum(interface_block):
    configdir = tempfile.mkdtemp(prefix="rns-scenario-")
    with open(os.path.join(configdir, "config"), "w") as f:
        f.write(
            "[reticulum]\n"
            "  enable_transport = False\n"
            "  share_instance = No\n"
            "  panic_on_interface_error = No\n"
            "[logging]\n"
            "  loglevel = 0\n"
            "[interfaces]\n" + interface_block
        )
    RNS.Reticulum(configdir=configdir)


def respond(name):
    port = free_port()
    start_reticulum(
        "  [[Bench TCP Server]]\n"
        "    type = TCPServerInterface\n"
        "    enabled = True\n"
        "    listen_ip = 127.0.0.1\n"
        f"    listen_port = {port}\n"
    )
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)

    state = {"delivered": 0, "payload_bytes": 0}
    done = threading.Event()

    def on_packet(message, packet):
        state["delivered"] += 1
        state["payload_bytes"] += len(message)

    def on_link(link):
        link.set_packet_callback(on_packet)
        link.set_link_closed_callback(lambda _link: done.set())

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr=127.0.0.1:{port}", flush=True)
    while not done.is_set():
        destination.announce()
        done.wait(ANNOUNCE_EVERY)
    print(
        f"RESULT delivered={state['delivered']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate(name, addr, profile, duration):
    host, port = addr.rsplit(":", 1)
    start_reticulum(
        "  [[Bench TCP Client]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = True\n"
        f"    target_host = {host}\n"
        f"    target_port = {port}\n"
    )

    heard = {"hash": None, "identity": None}
    announced = threading.Event()

    class Handler:
        aspect_filter = f"bench.{name}"

        def received_announce(self, destination_hash, announced_identity, app_data):
            heard["hash"] = destination_hash
            heard["identity"] = announced_identity
            announced.set()

    RNS.Transport.register_announce_handler(Handler())
    print("READY role=initiator", flush=True)
    if not announced.wait(30):
        sys.exit("no announce heard")

    destination = RNS.Destination(
        heard["identity"], RNS.Destination.OUT, RNS.Destination.SINGLE, "bench", name
    )
    up = threading.Event()
    link = RNS.Link(destination, established_callback=lambda _l: up.set())
    if not up.wait(30):
        sys.exit("link did not establish")

    payload = bytes([0xAB]) * profile["payload_len"]
    state = {"sent": 0, "delivered": 0, "timeouts": 0}
    rtts = []
    started = time.monotonic()
    deadline = started + duration
    drain_deadline = deadline + DRAIN_GRACE

    # Receipts are POLLED, not callback-driven: on localhost the proof can conclude a
    # receipt before a delivery callback could even be registered, and the reference
    # never fires callbacks retroactively.
    def send_one():
        state["sent"] += 1
        return RNS.Packet(link, payload).send()

    outstanding = [send_one() for _ in range(profile["window"])]
    while outstanding and time.monotonic() < drain_deadline:
        still = []
        for receipt in outstanding:
            status = receipt.status if receipt else RNS.PacketReceipt.FAILED
            if status == RNS.PacketReceipt.DELIVERED:
                state["delivered"] += 1
                rtts.append(receipt.get_rtt() * 1000.0)
                if time.monotonic() < deadline:
                    still.append(send_one())
            elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                state["timeouts"] += 1
                if time.monotonic() < deadline:
                    still.append(send_one())
            else:
                still.append(receipt)
        outstanding = still
        time.sleep(0.0005)
    elapsed_ms = int((time.monotonic() - started) * 1000)
    link.teardown()
    time.sleep(0.5)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    payload_bytes = state["delivered"] * profile["payload_len"]
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={state['sent']} delivered={state['delivered']} "
        f"timeouts={state['timeouts']} payload_bytes={payload_bytes} "
        f"elapsed_ms={elapsed_ms} delivered_per_sec={state['delivered'] / seconds:.1f} "
        f"goodput_bytes_per_sec={payload_bytes / seconds:.0f} "
        f"rtt_p50_ms={pct(0.50):.0f} rtt_p99_ms={pct(0.99):.0f}",
        flush=True,
    )
    os._exit(0)


def main():
    usage = "usage: scenario_node.py <manifest.json> <responder|initiator> <addr> [duration-ms]"
    if len(sys.argv) < 4:
        sys.exit(usage)
    with open(sys.argv[1]) as f:
        manifest = json.load(f)
    role, addr = sys.argv[2], sys.argv[3]
    duration_ms = int(sys.argv[4]) if len(sys.argv) > 4 else manifest["profile"]["duration_ms"]

    if role == "responder":
        respond(manifest["name"])
    elif role == "initiator":
        initiate(manifest["name"], addr, manifest["profile"], duration_ms / 1000.0)
    else:
        sys.exit(usage)


if __name__ == "__main__":
    main()
