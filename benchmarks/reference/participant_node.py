#!/usr/bin/env python3
"""The pinned RNS reference participant for live scenarios - the same contract as
`participant_node`: `participant_node.py <manifest.json> <role> <addr> [duration-ms]`,
READY/RESULT lines on stdout. The responder serves a ProveAll destination over a real
TCPServerInterface; the initiator connects with a TCPClientInterface and pumps windowed
SINGLE packets, measuring from the reference's own packet receipts."""

import json
import os
import queue
import socket
import sys
import tempfile
import threading
import time
from collections import deque

import RNS
from receipt_settlement import ReceiptSettlementQueue
from workload_vectors import (
    DEFAULT_SIZE_SEED,
    SizeSequence,
    deterministic_payload,
    repeated_payload,
)

ANNOUNCE_EVERY = 0.5
INITIATOR_COUNT = 1
DRAIN_GRACE = 5.0
QUIET_AFTER_TRAFFIC = 1.5
REQUEST_PATH = "/bench/query"
RESOURCE_ACK_PREFIX = b"PRNSRACK"
MAX_RESOURCE_BLOCK = 1024 * 1024 - 1


def auto_compress_from(profile):
    """The manifest's compression posture: "off" is the transport-only baseline,
    "auto" is RNS's shipping default (auto_compress=True)."""
    posture = profile.get("compression", "off")
    if posture == "off":
        return False
    if posture == "auto":
        return True
    sys.exit(f"unknown compression posture {posture!r} (expected 'off' or 'auto')")


def scenario_payload(profile, length):
    """Return the manifest-owned deterministic payload shape."""
    shape = profile.get("payload_shape", "dense")
    if shape == "dense":
        return deterministic_payload(length)
    if shape == "compressible":
        return deterministic_payload((length + 1) // 2).hex().encode()[:length]
    sys.exit(f"unknown payload shape {shape!r} (expected 'dense' or 'compressible')")


def resource_payload(profile, length):
    """Match Prns's bounded-memory stream: repeat one deterministic maximum
    resource block for the full logical transfer."""
    block = scenario_payload(profile, min(length, MAX_RESOURCE_BLOCK))
    return repeated_payload(block, length)


def await_measurement_start():
    print("MEASURE_READY", flush=True)
    command = sys.stdin.readline().strip()
    if command != "START":
        sys.exit(f"expected START measurement command, received {command!r}")


def await_startup_go():
    """Keep the responder silent until the runner confirms both process-local
    interfaces finished initialization. This avoids sending an announce into a
    reference TCP client while its constructor is still installing IFAC state."""
    command = sys.stdin.readline().strip()
    if command != "STARTUP":
        sys.exit(f"expected STARTUP command, received {command!r}")


def read_collection_target():
    fields = sys.stdin.readline().strip().split()
    if len(fields) != 3 or fields[0] != "COLLECT":
        sys.exit(f"expected COLLECT count bytes command, received {fields!r}")
    return int(fields[1]), int(fields[2])


def await_collection_release():
    command = sys.stdin.readline().strip()
    if command != "COLLECTED":
        sys.exit(f"expected COLLECTED release command, received {command!r}")


def sizes_from(profile, lo_key, hi_key, fixed_key, seed_xor=0):
    return SizeSequence(
        profile.get("size_seed", DEFAULT_SIZE_SEED) ^ seed_xor,
        profile.get(lo_key, 0),
        profile.get(hi_key, 0),
        profile.get(fixed_key, 0),
    )


def free_port():
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def relay_blocks(profile):
    port_a, port_b = free_port(), free_port()
    while port_b == port_a:
        port_b = free_port()
    bitrate = profile.get("tcp_bitrate_bps")
    bitrate_line = f"    bitrate = {bitrate}\n" if bitrate else ""
    block = (
        "  [[Relay Side A]]\n"
        "    type = TCPServerInterface\n"
        "    enabled = True\n"
        "    listen_ip = 127.0.0.1\n"
        f"    listen_port = {port_a}\n"
        + bitrate_line
        + "  [[Relay Side B]]\n"
        + "    type = TCPServerInterface\n"
        + "    enabled = True\n"
        + "    listen_ip = 127.0.0.1\n"
        + f"    listen_port = {port_b}\n"
        + bitrate_line
    )
    return block, f"127.0.0.1:{port_a}>127.0.0.1:{port_b}"


def run_relay(profile):
    block, ready_addr = relay_blocks(profile)
    configdir = tempfile.mkdtemp(prefix="rns-raw-transport-relay-")
    config = (
        "[reticulum]\n"
        "  enable_transport = True\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "[logging]\n"
        f"  loglevel = {os.environ.get('RNS_BENCH_LOGLEVEL', '0')}\n"
        "[interfaces]\n" + block
    )
    with open(os.path.join(configdir, "config"), "w") as f:
        f.write(config)
    RNS.Reticulum(configdir=configdir)
    relay_interfaces = [
        interface
        for interface in RNS.Transport.interfaces
        if getattr(interface, "name", None) in ("Relay Side A", "Relay Side B")
    ]
    policies = {
        (int(interface.bitrate), int(interface.HW_MTU))
        for interface in relay_interfaces
    }
    if len(relay_interfaces) != 2 or len(policies) != 1:
        sys.exit(
            "reference relay TCP policy did not resolve identically on both benchmark sides"
        )
    bitrate_bps, mtu_bytes = policies.pop()
    print(
        f"READY role=relay addr={ready_addr} "
        f"bitrate_bps={bitrate_bps} mtu_bytes={mtu_bytes}",
        flush=True,
    )
    command = sys.stdin.readline().strip()
    if command != "STOP":
        sys.exit(f"expected STOP relay command, received {command!r}")
    os._exit(0)


def interface_block(wire, role, addr, fixed_mtu=None, tcp_bitrate_bps=None):
    """One role's interface config plus the address its READY line should carry. UDP is
    symmetric (the orchestrator pre-assigns both ends as local>peer, the reference's
    fixed listen/forward model); TCP keeps the listen-then-connect flow."""
    if wire == "udp":
        local, peer = addr.split(">")
        local_host, local_port = local.rsplit(":", 1)
        peer_host, peer_port = peer.rsplit(":", 1)
        return (
            "  [[Bench UDP]]\n"
            "    type = UDPInterface\n"
            "    enabled = True\n"
            f"    listen_ip = {local_host}\n"
            f"    listen_port = {local_port}\n"
            f"    forward_ip = {peer_host}\n"
            f"    forward_port = {peer_port}\n"
        ), addr
    if role == "responder":
        port = free_port()
        mtu_line = f"    fixed_mtu = {fixed_mtu}\n" if fixed_mtu else ""
        bitrate_line = (
            f"    bitrate = {tcp_bitrate_bps}\n" if tcp_bitrate_bps else ""
        )
        return (
            "  [[Bench TCP Server]]\n"
            "    type = TCPServerInterface\n"
            "    enabled = True\n"
            "    listen_ip = 127.0.0.1\n"
            f"    listen_port = {port}\n"
            + mtu_line
            + bitrate_line
        ), f"127.0.0.1:{port}"
    host, port = addr.rsplit(":", 1)
    mtu_line = f"    fixed_mtu = {fixed_mtu}\n" if fixed_mtu else ""
    bitrate_line = f"    bitrate = {tcp_bitrate_bps}\n" if tcp_bitrate_bps else ""
    return (
        "  [[Bench TCP Client]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = True\n"
        f"    target_host = {host}\n"
        f"    target_port = {port}\n"
        + mtu_line
        + bitrate_line
    ), addr


def start_reticulum(interface_block):
    configdir = tempfile.mkdtemp(prefix="rns-scenario-")
    config = (
        "[reticulum]\n"
        "  enable_transport = False\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "[logging]\n"
        f"  loglevel = {os.environ.get('RNS_BENCH_LOGLEVEL', '0')}\n"
        "[interfaces]\n" + interface_block
    )
    with open(os.path.join(configdir, "config"), "w") as f:
        f.write(config)
    RNS.Reticulum(configdir=configdir)


def respond(name, block, ready_addr, _profile):
    start_reticulum(block)
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
        state["last_delivery"] = time.monotonic()

    state["last_delivery"] = None
    destination.set_packet_callback(on_packet)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    await_startup_go()
    print("MEASURE_READY", flush=True)
    while True:
        if state["delivered"] == 0:
            destination.announce()
        done.wait(ANNOUNCE_EVERY)
        last = state["last_delivery"]
        if last is not None and time.monotonic() - last > QUIET_AFTER_TRAFFIC:
            break
    print(
        f"RESULT delivered={state['delivered']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate(name, block, profile, duration):
    start_reticulum(block)

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
    sizes = sizes_from(profile, "payload_min", "payload_max", "payload_len")
    scratch = scenario_payload(
        profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0))
    )
    payloads = tuple(scratch[:size] for size in range(len(scratch) + 1))
    state = {"sent": 0, "delivered": 0, "timeouts": 0, "delivered_bytes": 0}
    rtts = []
    await_measurement_start()
    started = time.monotonic()
    deadline = started + duration
    drain_deadline = deadline + DRAIN_GRACE

    settlement = ReceiptSettlementQueue()
    outstanding = {}

    def send_one():
        state["sent"] += 1
        size = sizes.next_len()
        receipt = RNS.Packet(destination, payloads[size]).send()
        armed = settlement.arm(receipt, RNS.PacketReceipt.SENT, size)
        outstanding[id(armed)] = armed

    for _ in range(profile["window"]):
        send_one()
    streak_limit = max(profile["window"] * 8, 64)
    failure_streak = 0
    died = False
    while outstanding and time.monotonic() < drain_deadline:
        armed = settlement.pop_until(drain_deadline)
        if armed is None:
            break
        outstanding.pop(id(armed), None)
        receipt = armed.receipt
        size = armed.context
        status = receipt.status if receipt else RNS.PacketReceipt.FAILED
        if status == RNS.PacketReceipt.DELIVERED:
            state["delivered"] += 1
            state["delivered_bytes"] += size
            rtts.append(receipt.get_rtt() * 1000.0)
            failure_streak = 0
        elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
            state["timeouts"] += 1
            failure_streak += 1
            if not died and failure_streak >= streak_limit:
                died = True
                print(f"DIED failure_streak={failure_streak}", file=sys.stderr, flush=True)
        else:
            sys.exit(f"settlement callback carried pending receipt status {status}")
        if not died and time.monotonic() < deadline:
            send_one()
    state["timeouts"] += len(outstanding)
    elapsed_ms = int((time.monotonic() - started) * 1000)
    print("MEASURE_DONE", flush=True)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    payload_bytes = state["delivered_bytes"]
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT attempted={state['sent']} sent={state['sent']} delivered={state['delivered']} "
        f"timeouts={state['timeouts']} payload_bytes={payload_bytes} "
        f"elapsed_ms={elapsed_ms} delivered_per_sec={state['delivered'] / seconds:.1f} "
        f"goodput_bytes_per_sec={payload_bytes / seconds:.0f} "
        f"rtt_p50_ms={pct(0.50):.0f} rtt_p99_ms={pct(0.99):.0f}"
        + (" died=1" if died else ""),
        flush=True,
    )
    os._exit(0)


def respond_link(name, block, ready_addr, _profile):
    start_reticulum(block)
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

    links = {"up": 0, "closed": 0}
    links_lock = threading.Lock()

    def on_closed(_link):
        with links_lock:
            links["closed"] += 1
        if links["closed"] >= INITIATOR_COUNT:
            done.set()

    def on_link(link):
        link.set_packet_callback(on_packet)
        link.set_link_closed_callback(on_closed)
        with links_lock:
            links["up"] += 1
            if links["up"] == INITIATOR_COUNT:
                print("MEASURE_READY", flush=True)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    await_startup_go()
    while not done.is_set():
        if links["up"] < INITIATOR_COUNT:
            destination.announce()
        done.wait(ANNOUNCE_EVERY)
    print(
        f"RESULT delivered={state['delivered']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate_link(name, block, profile, duration):
    start_reticulum(block)

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

    sizes = sizes_from(profile, "payload_min", "payload_max", "payload_len")
    scratch = scenario_payload(
        profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0))
    )
    payloads = tuple(scratch[:size] for size in range(len(scratch) + 1))
    state = {
        "sent": 0,
        "sent_bytes": 0,
        "receipt_proved": 0,
        "receipt_unproved": 0,
    }
    rtts = []
    await_measurement_start()
    started = time.monotonic()
    deadline = started + duration
    drain_deadline = deadline + DRAIN_GRACE

    settlement = ReceiptSettlementQueue()
    outstanding = {}

    def send_one():
        state["sent"] += 1
        size = sizes.next_len()
        state["sent_bytes"] += size
        receipt = RNS.Packet(link, payloads[size]).send()
        armed = settlement.arm(receipt, RNS.PacketReceipt.SENT, size)
        outstanding[id(armed)] = armed

    for _ in range(profile["window"]):
        send_one()
    streak_limit = max(profile["window"] * 8, 64)
    failure_streak = 0
    died = False
    while outstanding and time.monotonic() < drain_deadline:
        armed = settlement.pop_until(drain_deadline)
        if armed is None:
            break
        outstanding.pop(id(armed), None)
        receipt = armed.receipt
        status = receipt.status if receipt else RNS.PacketReceipt.FAILED
        if status == RNS.PacketReceipt.DELIVERED:
            state["receipt_proved"] += 1
            rtts.append(receipt.get_rtt() * 1000.0)
            failure_streak = 0
        elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
            state["receipt_unproved"] += 1
            failure_streak += 1
            if not died and failure_streak >= streak_limit:
                died = True
                print(f"DIED failure_streak={failure_streak}", file=sys.stderr, flush=True)
        else:
            sys.exit(f"settlement callback carried pending receipt status {status}")
        if not died and time.monotonic() < deadline:
            send_one()
    state["receipt_unproved"] += len(outstanding)
    elapsed_ms = int((time.monotonic() - started) * 1000)
    print("MEASURE_DONE", flush=True)
    link.teardown()
    time.sleep(0.5)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    payload_bytes = state["sent_bytes"]
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT attempted={state['sent']} sent={state['sent']} delivered={state['sent']} "
        f"timeouts=0 receipt_proved={state['receipt_proved']} "
        f"receipt_unproved={state['receipt_unproved']} payload_bytes={payload_bytes} "
        f"elapsed_ms={elapsed_ms} delivered_per_sec={state['sent'] / seconds:.1f} "
        f"goodput_bytes_per_sec={payload_bytes / seconds:.0f} "
        f"rtt_p50_ms={pct(0.50):.0f} rtt_p99_ms={pct(0.99):.0f}"
        + (" died=1" if died else ""),
        flush=True,
    )
    os._exit(0)


def respond_resource(name, block, ready_addr, profile):
    """The accepting end of the bulk mechanism: ACCEPT_ALL on every inbound
    link, count each hash-proved transfer at its application conclusion, and
    report only when the runner's exact collection target has arrived."""
    start_reticulum(block)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)

    state = {"received": 0, "payload_bytes": 0}
    state_changed = threading.Condition()
    acknowledgement_queue = queue.SimpleQueue()

    def send_acknowledgements():
        while True:
            link, sequence = acknowledgement_queue.get()
            RNS.Packet(link, RESOURCE_ACK_PREFIX + sequence.to_bytes(8, "big")).send()

    threading.Thread(target=send_acknowledgements, daemon=True).start()

    def on_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            data = resource.data.read()
            with state_changed:
                state["received"] += 1
                state["payload_bytes"] += len(data)
                sequence = state["received"]
                state_changed.notify_all()
            acknowledgement_queue.put((resource.link, sequence))

    links = {"up": 0}
    links_lock = threading.Lock()

    def on_link(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(on_concluded)
        with links_lock:
            links["up"] += 1
            if links["up"] == INITIATOR_COUNT:
                print("MEASURE_READY", flush=True)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    await_startup_go()

    collection = {"target": None}

    def read_target():
        target = read_collection_target()
        with state_changed:
            collection["target"] = target
            state_changed.notify_all()

    threading.Thread(target=read_target, daemon=True).start()
    while links["up"] < INITIATOR_COUNT:
        destination.announce()
        time.sleep(ANNOUNCE_EVERY)

    deadline = None
    with state_changed:
        while True:
            if collection["target"] is None:
                state_changed.wait(ANNOUNCE_EVERY)
                continue
            if deadline is None:
                deadline = time.monotonic() + profile.get("drain_timeout_ms", 30_000) / 1000.0
            if collection["target"] == (
                state["received"],
                state["payload_bytes"],
            ):
                break
            left = deadline - time.monotonic()
            if left <= 0:
                break
            state_changed.wait(min(left, ANNOUNCE_EVERY))
    print(
        f"RESULT received={state['received']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate_resource(name, block, profile, duration):
    """The measuring end: one link, then maximum-size resources back to back
    until the wall-time elapses — incompressible payload with compression work
    disabled, so the measurement is the resource/link machinery."""
    start_reticulum(block)

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

    acknowledgements = {"sequence": 0}
    acknowledgement_changed = threading.Condition()

    def on_ack(message, _packet):
        if len(message) == 16 and message[:8] == RESOURCE_ACK_PREFIX:
            with acknowledgement_changed:
                acknowledgements["sequence"] = max(
                    acknowledgements["sequence"], int.from_bytes(message[8:], "big")
                )
                acknowledgement_changed.notify_all()

    link.set_packet_callback(on_ack)

    sizes = sizes_from(profile, "payload_min", "payload_max", "payload_len")
    scratch = resource_payload(
        profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0))
    )
    state = {
        "sent": 0,
        "settled": 0,
        "protocol_failures": 0,
        "participant_timeouts": 0,
        "ack_timeouts": 0,
        "settled_bytes": 0,
    }
    transfer_ms = []
    await_measurement_start()
    started = time.monotonic()
    deadline = started + duration
    while time.monotonic() < deadline:
        concluded = threading.Event()
        outcome = {}

        def callback(resource):
            outcome["status"] = resource.status
            outcome["resource"] = resource
            concluded.set()

        state["sent"] += 1
        size = sizes.next_len()
        transfer_started = time.monotonic()
        RNS.Resource(scratch[:size], link, auto_compress=auto_compress_from(profile), callback=callback)
        if not concluded.wait(120):
            state["participant_timeouts"] += 1
            print(
                f"RESOURCE_FAILURE kind=participant-timeout sequence={state['sent']} "
                "wait_ms=120000",
                file=sys.stderr,
                flush=True,
            )
            break
        if outcome["status"] == RNS.Resource.COMPLETE:
            state["settled"] += 1
            state["settled_bytes"] += size
            transfer_ms.append((time.monotonic() - transfer_started) * 1000.0)
        else:
            state["protocol_failures"] += 1
            failed_resource = outcome["resource"]
            status_names = {
                RNS.Resource.NONE: "NONE",
                RNS.Resource.QUEUED: "QUEUED",
                RNS.Resource.ADVERTISED: "ADVERTISED",
                RNS.Resource.TRANSFERRING: "TRANSFERRING",
                RNS.Resource.AWAITING_PROOF: "AWAITING_PROOF",
                RNS.Resource.ASSEMBLING: "ASSEMBLING",
                RNS.Resource.COMPLETE: "COMPLETE",
                RNS.Resource.FAILED: "FAILED",
                RNS.Resource.CORRUPT: "CORRUPT",
                RNS.Resource.REJECTED: "REJECTED",
            }
            resource_hash = getattr(failed_resource, "hash", b"")
            print(
                f"RESOURCE_FAILURE kind=protocol sequence={state['sent']} "
                f"status={outcome['status']} "
                f"status_name={status_names.get(outcome['status'], 'UNKNOWN')} "
                f"retries_left={getattr(failed_resource, 'retries_left', 'unknown')} "
                f"max_retries={getattr(failed_resource, 'max_retries', 'unknown')} "
                f"hash={resource_hash.hex() if resource_hash else 'unknown'}",
                file=sys.stderr,
                flush=True,
            )
            break
        ack_deadline = time.monotonic() + profile.get("drain_timeout_ms", 30_000) / 1000.0
        with acknowledgement_changed:
            while acknowledgements["sequence"] < state["sent"]:
                left = ack_deadline - time.monotonic()
                if left <= 0:
                    state["ack_timeouts"] += 1
                    print(
                        f"RESOURCE_FAILURE kind=application-ack-timeout "
                        f"sequence={state['sent']} "
                        f"last_ack={acknowledgements['sequence']} "
                        f"wait_ms={profile.get('drain_timeout_ms', 30_000)}",
                        file=sys.stderr,
                        flush=True,
                    )
                    break
                acknowledgement_changed.wait(left)
            if acknowledgements["sequence"] < state["sent"]:
                break
    elapsed_ms = int((time.monotonic() - started) * 1000)
    print("MEASURE_DONE", flush=True)

    transfer_ms = sorted(transfer_ms)
    pct = lambda p: (
        transfer_ms[min(round((len(transfer_ms) - 1) * p), len(transfer_ms) - 1)]
        if transfer_ms
        else float("nan")
    )
    payload_bytes = state["settled_bytes"]
    failures = (
        state["protocol_failures"]
        + state["participant_timeouts"]
        + state["ack_timeouts"]
    )
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={state['sent']} settled={state['settled']} "
        f"failures={failures} protocol_failures={state['protocol_failures']} "
        f"participant_timeouts={state['participant_timeouts']} "
        f"ack_timeouts={state['ack_timeouts']} payload_bytes={payload_bytes} "
        f"elapsed_ms={elapsed_ms} "
        f"goodput_bytes_per_sec={payload_bytes / seconds:.0f} "
        f"goodput_mbits_per_sec={payload_bytes * 8.0 / seconds / 1e6:.2f} "
        f"transfer_p50_ms={pct(0.50):.0f} transfer_p99_ms={pct(0.99):.0f}",
        flush=True,
    )
    await_collection_release()
    link.teardown()
    os._exit(0)


def respond_request(name, block, ready_addr, profile):
    """The serving end of the request shape: the registered handler answers every
    allowed request with exactly the byte count the request names."""
    start_reticulum(block)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {"served": 0, "response_bytes": 0}
    done = threading.Event()
    scratch = scenario_payload(profile, profile["response_max"])

    def answer(path, data, request_id, link_id, remote_identity, requested_at):
        wanted = int.from_bytes(data[:2], "big") if data and len(data) >= 2 else 0
        wanted = min(wanted, len(scratch))
        if data[2:6] != b"WARM":
            state["served"] += 1
            state["response_bytes"] += wanted
        return scratch[:wanted]

    destination.register_request_handler(
        REQUEST_PATH, response_generator=answer, allow=RNS.Destination.ALLOW_ALL
    )

    links = {"up": 0, "closed": 0}
    links_lock = threading.Lock()

    expected_links = profile.get("request_links", profile["window"])

    def on_closed(_link):
        with links_lock:
            links["closed"] += 1
        if links["closed"] >= expected_links:
            done.set()

    def on_link(link):
        link.set_link_closed_callback(on_closed)
        with links_lock:
            links["up"] += 1
            if links["up"] == expected_links:
                print("MEASURE_READY", flush=True)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    await_startup_go()
    while not done.is_set():
        if links["up"] < INITIATOR_COUNT:
            destination.announce()
        done.wait(ANNOUNCE_EVERY)
    time.sleep(0.5)
    print(
        f"RESULT served={state['served']} response_bytes={state['response_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate_request(name, block, profile, duration):
    """The asking end: windowed requests of varied sizes, each naming the
    varied response size it wants back."""
    start_reticulum(block)

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
    links = []
    request_links = profile.get("request_links", profile["window"])
    for _ in range(request_links):
        up = threading.Event()
        link = RNS.Link(destination, established_callback=lambda _link, ready=up: ready.set())
        if not up.wait(30):
            sys.exit("request link did not establish")
        links.append(link)

    scratch = scenario_payload(profile, max(profile.get("request_max", 2), 2))
    warm_len = profile["request_min"]
    warm_request = (
        profile["response_min"].to_bytes(2, "big")
        + b"WARM"
        + scratch[: warm_len - 6]
    )
    for index, link in enumerate(links, start=1):
        armed = False
        for attempt in range(1, 4):
            done = threading.Event()
            outcome = {"response": None}

            def warm_response(receipt):
                outcome["response"] = receipt.response
                done.set()

            receipt = link.request(
                REQUEST_PATH,
                warm_request,
                response_callback=warm_response,
                failed_callback=lambda _receipt: done.set(),
                timeout=5.0,
            )
            if receipt:
                done.wait(6.0)
            armed = (
                outcome["response"] is not None
                and len(outcome["response"]) == profile["response_min"]
            )
            print(
                f"STARTUP_ATTEMPT stage=request-link-arm link={index} "
                f"attempt={attempt} result={'pass' if armed else 'fail'}",
                flush=True,
            )
            if armed:
                break
        if not armed:
            sys.exit(f"request link {index} did not arm after three public-API attempts")

    request_sizes = sizes_from(profile, "request_min", "request_max", "request_min")
    response_sizes = sizes_from(
        profile, "response_min", "response_max", "response_min", seed_xor=0xA5A5A5A5A5A5A5A5
    )
    state = {
        "sent": 0,
        "delivered": 0,
        "timeouts": 0,
        "request_bytes": 0,
        "response_bytes": 0,
        "expected_response_bytes": 0,
        "in_flight": 0,
    }
    rtts = []
    state_changed = threading.Condition()
    available_links = deque(links)
    pending_receipts = {}
    started = None
    deadline = None

    def on_response(receipt):
        with state_changed:
            pending_receipts.pop(id(receipt), None)
            state["delivered"] += 1
            state["in_flight"] -= 1
            available_links.append(receipt.link)
            state["response_bytes"] += len(receipt.response or b"")
            rtts.append((time.monotonic() - receipt.sent_at_wall) * 1000.0)
            state_changed.notify()

    def on_failed(receipt):
        with state_changed:
            pending_receipts.pop(id(receipt), None)
            state["timeouts"] += 1
            state["in_flight"] -= 1
            available_links.append(receipt.link)
            state_changed.notify()

    def send_one(link):
        request_len = max(request_sizes.next_len(), 2)
        wanted = response_sizes.next_len()
        data = wanted.to_bytes(2, "big") + scratch[: request_len - 2]
        state["sent"] += 1
        state["request_bytes"] += request_len
        state["expected_response_bytes"] += wanted
        state["in_flight"] += 1
        receipt = link.request(
            REQUEST_PATH,
            data,
            response_callback=on_response,
            failed_callback=on_failed,
            timeout=profile.get("drain_timeout_ms", 30000) / 1000.0,
        )
        if not receipt:
            available_links.append(link)
            state["sent"] -= 1
            state["request_bytes"] -= request_len
            state["expected_response_bytes"] -= wanted
            state["in_flight"] -= 1
            return
        receipt.sent_at_wall = time.monotonic()
        pending_receipts[id(receipt)] = receipt

    await_measurement_start()
    started = time.monotonic()
    deadline = started + duration
    with state_changed:
        for _ in range(profile["window"]):
            send_one(available_links.popleft())
    with state_changed:
        while True:
            in_flight = state["in_flight"]
            if in_flight < profile["window"] and available_links and time.monotonic() < deadline:
                send_one(available_links.popleft())
                continue
            if in_flight == 0:
                break
            state_changed.wait()
    elapsed_ms = int((time.monotonic() - started) * 1000)
    with state_changed:
        pending = len(pending_receipts)
        receiving = sum(
            receipt.status == RNS.RequestReceipt.RECEIVING
            for receipt in pending_receipts.values()
        )
        sent_pending = sum(
            receipt.status in (RNS.RequestReceipt.SENT, RNS.RequestReceipt.DELIVERED)
            for receipt in pending_receipts.values()
        )
        result_state = dict(state)
        result_state["timeouts"] += pending
    print("MEASURE_DONE", flush=True)
    for link in links:
        link.teardown()
    time.sleep(0.5)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={result_state['sent']} delivered={result_state['delivered']} "
        f"timeouts={result_state['timeouts']} raced=0 pending={pending} "
        f"pending_receiving={receiving} pending_sent={sent_pending} "
        f"request_bytes={result_state['request_bytes']} "
        f"response_bytes={result_state['response_bytes']} "
        f"expected_response_bytes={result_state['expected_response_bytes']} elapsed_ms={elapsed_ms} "
        f"requests_per_sec={result_state['delivered'] / seconds:.1f} "
        f"rtt_p50_ms={pct(0.50):.3f} rtt_p99_ms={pct(0.99):.3f} "
        f"request_window={profile['window']} request_links={len(links)}",
        flush=True,
    )
    os._exit(0)


def main():
    usage = "usage: participant_node.py <manifest.json> <responder|initiator> <addr> [duration-ms]"
    if len(sys.argv) < 4:
        sys.exit(usage)
    with open(sys.argv[1]) as f:
        manifest = json.load(f)
    role, addr = sys.argv[2], sys.argv[3]
    duration_ms = int(sys.argv[4]) if len(sys.argv) > 4 else manifest["profile"]["duration_ms"]

    global ANNOUNCE_EVERY, INITIATOR_COUNT, DRAIN_GRACE
    ANNOUNCE_EVERY = manifest["profile"].get("announce_every_ms", 500) / 1000.0
    INITIATOR_COUNT = int(manifest["profile"].get("initiator_count", 1))
    DRAIN_GRACE = manifest["profile"].get("drain_timeout_ms", 30000) / 1000.0

    mechanism = manifest["profile"]["mechanism"]
    wire = manifest["profile"].get("wire", "tcp")
    if role == "relay":
        if mechanism not in ("transport", "transport-resource"):
            sys.exit("reference relay role requires a transport mechanism")
        run_relay(manifest["profile"])
    if role not in ("responder", "initiator"):
        sys.exit(usage)
    block, ready_addr = interface_block(
        wire,
        role,
        addr,
        manifest["profile"].get("link_mtu"),
        manifest["profile"].get("tcp_bitrate_bps"),
    )
    responders = {
        "single": respond,
        "link": respond_link,
        "resource": respond_resource,
        "request": respond_request,
    }
    initiators = {
        "single": initiate,
        "link": initiate_link,
        "resource": initiate_resource,
        "request": initiate_request,
    }
    if role == "responder":
        handler = responders.get(mechanism)
        if handler is None:
            sys.exit(f"reference node has no responder for mechanism {mechanism!r}")
        handler(manifest["name"], block, ready_addr, manifest["profile"])
    else:
        handler = initiators.get(mechanism)
        if handler is None:
            sys.exit(f"reference node has no initiator for mechanism {mechanism!r}")
        handler(manifest["name"], block, manifest["profile"], duration_ms / 1000.0)


if __name__ == "__main__":
    main()
