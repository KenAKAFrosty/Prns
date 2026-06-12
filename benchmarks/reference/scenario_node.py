#!/usr/bin/env python3
"""RNS 1.3.1's participation binary for live scenarios — the same contract as
`scenario_node`: `scenario_node.py <manifest.json> <role> <addr> [duration-ms]`,
READY/RESULT lines on stdout. The responder serves a ProveAll destination over a real
TCPServerInterface; the initiator connects with a TCPClientInterface and pumps windowed
SINGLE packets, measuring from the reference's own packet receipts."""

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
QUIET_AFTER_TRAFFIC = 1.5
REQUEST_PATH = "/bench/query"
DEFAULT_SIZE_SEED = 0x5EEDCAFEF00D0001
MASK64 = 0xFFFFFFFFFFFFFFFF


class SizeSequence:
    """The varied-size law every node speaks identically: a seeded xorshift
    draws each message's size in [min, max] — the same sequence the Rust node
    draws, so byte totals stay comparable without exchanging anything."""

    def __init__(self, seed, lo, hi, fixed):
        if not hi:
            lo, hi = fixed, fixed
        self.state = seed & MASK64
        self.lo = lo
        self.hi = hi

    def next_len(self):
        return self.next_in(self.lo, self.hi)

    def next_in(self, lo, hi):
        s = self.state
        s = (s ^ (s << 13)) & MASK64
        s = (s ^ (s >> 7)) & MASK64
        s = (s ^ (s << 17)) & MASK64
        self.state = s
        return lo + (s % (hi - lo + 1))


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


def relay_blocks():
    port_a, port_b = free_port(), free_port()
    block = (
        "  [[Relay Side A]]\n"
        "    type = TCPServerInterface\n"
        "    enabled = True\n"
        "    listen_ip = 127.0.0.1\n"
        f"    listen_port = {port_a}\n"
        "  [[Relay Side B]]\n"
        "    type = TCPServerInterface\n"
        "    enabled = True\n"
        "    listen_ip = 127.0.0.1\n"
        f"    listen_port = {port_b}\n"
    )
    return block, f"127.0.0.1:{port_a}>127.0.0.1:{port_b}"


def relay(name, _addr):
    """A pure transport node: enable_transport and nothing else."""
    block, ready_addr = relay_blocks()
    configdir = tempfile.mkdtemp(prefix="rns-scenario-relay-")
    with open(os.path.join(configdir, "config"), "w") as f:
        f.write(
            "[reticulum]\n"
            "  enable_transport = True\n"
            "  share_instance = No\n"
            "  panic_on_interface_error = No\n"
            "[logging]\n"
            f"  loglevel = {os.environ.get('RNS_BENCH_LOGLEVEL', '0')}\n"
            "[interfaces]\n" + block
        )
    RNS.Reticulum(configdir=configdir)
    print(f"READY role=relay addr={ready_addr}", flush=True)
    while True:
        time.sleep(3600)


def interface_block(wire, role, addr, topology="direct"):
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
    if role == "responder" and topology != "relay":
        port = free_port()
        return (
            "  [[Bench TCP Server]]\n"
            "    type = TCPServerInterface\n"
            "    enabled = True\n"
            "    listen_ip = 127.0.0.1\n"
            f"    listen_port = {port}\n"
        ), f"127.0.0.1:{port}"
    host, port = addr.rsplit(":", 1)
    return (
        "  [[Bench TCP Client]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = True\n"
        f"    target_host = {host}\n"
        f"    target_port = {port}\n"
    ), addr


def start_reticulum(interface_block):
    configdir = tempfile.mkdtemp(prefix="rns-scenario-")
    with open(os.path.join(configdir, "config"), "w") as f:
        f.write(
            "[reticulum]\n"
            "  enable_transport = False\n"
            "  share_instance = No\n"
            "  panic_on_interface_error = No\n"
            "[logging]\n"
            f"  loglevel = {os.environ.get('RNS_BENCH_LOGLEVEL', '0')}\n"
            "[interfaces]\n" + interface_block
        )
    RNS.Reticulum(configdir=configdir)


def respond(name, block, ready_addr):
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
    while True:
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
    scratch = os.urandom(max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
    state = {"sent": 0, "delivered": 0, "timeouts": 0, "delivered_bytes": 0}
    rtts = []
    started = time.monotonic()
    deadline = started + duration
    drain_deadline = deadline + DRAIN_GRACE

    # Receipts are POLLED, not callback-driven: on localhost the proof can conclude a
    # receipt before a delivery callback could even be registered, and the reference
    # never fires callbacks retroactively.
    def send_one():
        state["sent"] += 1
        size = sizes.next_len()
        return RNS.Packet(destination, scratch[:size]).send(), size

    outstanding = [send_one() for _ in range(profile["window"])]
    while outstanding and time.monotonic() < drain_deadline:
        still = []
        settled = 0
        for receipt, size in outstanding:
            status = receipt.status if receipt else RNS.PacketReceipt.FAILED
            if status == RNS.PacketReceipt.DELIVERED:
                state["delivered"] += 1
                state["delivered_bytes"] += size
                rtts.append(receipt.get_rtt() * 1000.0)
                settled += 1
                if time.monotonic() < deadline:
                    still.append(send_one())
            elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                state["timeouts"] += 1
                settled += 1
                if time.monotonic() < deadline:
                    still.append(send_one())
            else:
                still.append((receipt, size))
        outstanding = still
        if settled == 0:
            time.sleep(0.0005)
    elapsed_ms = int((time.monotonic() - started) * 1000)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    payload_bytes = state["delivered_bytes"]
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


def respond_link(name, block, ready_addr):
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

    def on_link(link):
        link.set_packet_callback(on_packet)
        link.set_link_closed_callback(lambda _link: done.set())

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    while not done.is_set():
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
    scratch = os.urandom(max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
    state = {"sent": 0, "delivered": 0, "timeouts": 0, "delivered_bytes": 0}
    rtts = []
    started = time.monotonic()
    deadline = started + duration
    drain_deadline = deadline + DRAIN_GRACE

    # Receipts are POLLED, not callback-driven: on localhost the proof can conclude a
    # receipt before a delivery callback could even be registered, and the reference
    # never fires callbacks retroactively.
    def send_one():
        state["sent"] += 1
        size = sizes.next_len()
        return RNS.Packet(link, scratch[:size]).send(), size

    outstanding = [send_one() for _ in range(profile["window"])]
    while outstanding and time.monotonic() < drain_deadline:
        still = []
        settled = 0
        for receipt, size in outstanding:
            status = receipt.status if receipt else RNS.PacketReceipt.FAILED
            if status == RNS.PacketReceipt.DELIVERED:
                state["delivered"] += 1
                state["delivered_bytes"] += size
                rtts.append(receipt.get_rtt() * 1000.0)
                settled += 1
                if time.monotonic() < deadline:
                    still.append(send_one())
            elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                state["timeouts"] += 1
                settled += 1
                if time.monotonic() < deadline:
                    still.append(send_one())
            else:
                still.append((receipt, size))
        outstanding = still
        if settled == 0:
            time.sleep(0.0005)
    elapsed_ms = int((time.monotonic() - started) * 1000)
    link.teardown()
    time.sleep(0.5)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    payload_bytes = state["delivered_bytes"]
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


def respond_resource(name, block, ready_addr):
    """The accepting end of the bulk mechanism: ACCEPT_ALL on every inbound
    link, count each hash-proved transfer at its conclusion, report when the
    initiator tears the link down."""
    start_reticulum(block)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)

    state = {"received": 0, "payload_bytes": 0}
    done = threading.Event()

    def on_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            state["received"] += 1
            data = resource.data.read()
            state["payload_bytes"] += len(data)

    def on_link(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(on_concluded)
        link.set_link_closed_callback(lambda _link: done.set())

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    while not done.is_set():
        destination.announce()
        done.wait(ANNOUNCE_EVERY)
    time.sleep(0.5)
    print(
        f"RESULT received={state['received']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate_resource(name, block, profile, duration):
    """The measuring end: one link, then maximum-size resources back to back
    until the wall-time elapses — incompressible payload so auto-compress
    keeps the full stream on the wire."""
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
    scratch = os.urandom(max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
    state = {"sent": 0, "settled": 0, "failures": 0, "settled_bytes": 0}
    transfer_ms = []
    started = time.monotonic()
    deadline = started + duration
    while time.monotonic() < deadline:
        concluded = threading.Event()
        outcome = {}

        def callback(resource):
            outcome["status"] = resource.status
            concluded.set()

        state["sent"] += 1
        size = sizes.next_len()
        transfer_started = time.monotonic()
        RNS.Resource(scratch[:size], link, callback=callback)
        if not concluded.wait(120):
            state["failures"] += 1
            break
        if outcome["status"] == RNS.Resource.COMPLETE:
            state["settled"] += 1
            state["settled_bytes"] += size
            transfer_ms.append((time.monotonic() - transfer_started) * 1000.0)
        else:
            state["failures"] += 1
    elapsed_ms = int((time.monotonic() - started) * 1000)
    link.teardown()
    time.sleep(0.5)

    transfer_ms = sorted(transfer_ms)
    pct = lambda p: (
        transfer_ms[min(round((len(transfer_ms) - 1) * p), len(transfer_ms) - 1)]
        if transfer_ms
        else float("nan")
    )
    payload_bytes = state["settled_bytes"]
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={state['sent']} settled={state['settled']} "
        f"failures={state['failures']} payload_bytes={payload_bytes} "
        f"elapsed_ms={elapsed_ms} "
        f"goodput_bytes_per_sec={payload_bytes / seconds:.0f} "
        f"goodput_mbits_per_sec={payload_bytes * 8.0 / seconds / 1e6:.2f} "
        f"transfer_p50_ms={pct(0.50):.0f} transfer_p99_ms={pct(0.99):.0f}",
        flush=True,
    )
    os._exit(0)


def respond_request(name, block, ready_addr):
    """The serving end of the RPC shape: the registered handler answers every
    allowed request with exactly the byte count the request names."""
    start_reticulum(block)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {"served": 0, "response_bytes": 0}
    done = threading.Event()
    scratch = os.urandom(512)

    def answer(path, data, request_id, link_id, remote_identity, requested_at):
        wanted = int.from_bytes(data[:2], "big") if data and len(data) >= 2 else 0
        wanted = min(wanted, len(scratch))
        state["served"] += 1
        state["response_bytes"] += wanted
        return scratch[:wanted]

    destination.register_request_handler(
        REQUEST_PATH, response_generator=answer, allow=RNS.Destination.ALLOW_ALL
    )

    def on_link(link):
        link.set_link_closed_callback(lambda _link: done.set())

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    while not done.is_set():
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
    up = threading.Event()
    link = RNS.Link(destination, established_callback=lambda _l: up.set())
    if not up.wait(30):
        sys.exit("link did not establish")

    request_sizes = sizes_from(profile, "request_min", "request_max", "request_min")
    response_sizes = sizes_from(
        profile, "response_min", "response_max", "response_min", seed_xor=0xA5A5A5A5A5A5A5A5
    )
    scratch = os.urandom(max(profile.get("request_max", 2), 2))
    state = {
        "sent": 0,
        "delivered": 0,
        "timeouts": 0,
        "request_bytes": 0,
        "response_bytes": 0,
        "in_flight": 0,
    }
    rtts = []
    lock = threading.Lock()
    settled = threading.Event()
    started = time.monotonic()
    deadline = started + duration

    second_counts = {}

    def on_response(receipt):
        with lock:
            state["delivered"] += 1
            state["in_flight"] -= 1
            state["response_bytes"] += len(receipt.response or b"")
            rtts.append((time.monotonic() - receipt.sent_at_wall) * 1000.0)
            bucket = int(time.monotonic() - started)
            second_counts[bucket] = second_counts.get(bucket, 0) + 1
        settled.set()

    def on_failed(receipt):
        with lock:
            state["timeouts"] += 1
            state["in_flight"] -= 1
        settled.set()

    def send_one():
        request_len = max(request_sizes.next_len(), 2)
        wanted = response_sizes.next_len()
        data = wanted.to_bytes(2, "big") + scratch[: request_len - 2]
        state["sent"] += 1
        state["request_bytes"] += request_len
        state["in_flight"] += 1
        receipt = link.request(
            REQUEST_PATH, data, response_callback=on_response, failed_callback=on_failed
        )
        receipt.sent_at_wall = time.monotonic()

    with lock:
        for _ in range(profile["window"]):
            send_one()
    drain_deadline = deadline + DRAIN_GRACE
    while time.monotonic() < drain_deadline:
        with lock:
            in_flight = state["in_flight"]
            if in_flight < profile["window"] and time.monotonic() < deadline:
                send_one()
                continue
            if in_flight == 0:
                break
        settled.wait(0.05)
        settled.clear()
    elapsed_ms = int((time.monotonic() - started) * 1000)
    print(
        "DEBUG pending=" + str(len(link.pending_requests))
        + " link_status=" + str(link.status)
        + " inactive_for=" + str(round(link.inactive_for(), 3))
        + " pending_ids=" + str([RNS.prettyhexrep(r.request_id) for r in link.pending_requests]),
        file=sys.stderr,
        flush=True,
    )
    link.teardown()
    time.sleep(0.5)
    print(
        "DEBUG per_second_settles=" + str(sorted(second_counts.items())),
        file=sys.stderr,
        flush=True,
    )

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={state['sent']} delivered={state['delivered']} "
        f"timeouts={state['timeouts']} request_bytes={state['request_bytes']} "
        f"response_bytes={state['response_bytes']} elapsed_ms={elapsed_ms} "
        f"requests_per_sec={state['delivered'] / seconds:.1f} "
        f"rtt_p50_ms={pct(0.50):.0f} rtt_p99_ms={pct(0.99):.0f}",
        flush=True,
    )
    os._exit(0)


def roll_band(sizes, profile):
    roll = sizes.next_in(0, 99)
    if roll < profile["command_share"]:
        return "command", sizes.next_in(profile["command_min"], profile["command_max"])
    if roll < profile["command_share"] + profile["page_share"]:
        return "page", sizes.next_in(profile["page_min"], profile["page_max"])
    return "file", sizes.next_in(profile["file_min"], profile["file_max"])


def respond_churn(name, block, ready_addr):
    """The serving end of session churn: ACCEPT_ALL and a packet callback on
    every fresh link; report after the churn has been quiet."""
    start_reticulum(block)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "bench", name
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)
    state = {"received": 0, "payload_bytes": 0, "last": None}

    def on_packet(message, packet):
        state["received"] += 1
        state["payload_bytes"] += len(message)
        state["last"] = time.monotonic()

    def on_concluded(resource):
        if resource.status == RNS.Resource.COMPLETE:
            state["received"] += 1
            state["payload_bytes"] += len(resource.data.read())
            state["last"] = time.monotonic()

    def on_link(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_packet_callback(on_packet)
        link.set_resource_concluded_callback(on_concluded)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    done = threading.Event()
    while True:
        destination.announce()
        done.wait(ANNOUNCE_EVERY)
        last = state["last"]
        if last is not None and time.monotonic() - last > QUIET_AFTER_TRAFFIC:
            break
    print(
        f"RESULT received={state['received']} payload_bytes={state['payload_bytes']}",
        flush=True,
    )
    os._exit(0)


def initiate_churn(name, block, profile, duration):
    """The churning end: whole sessions back to back — establish, move one
    banded payload, tear down."""
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
    sizes = SizeSequence(profile.get("size_seed", DEFAULT_SIZE_SEED), 0, 0, 1)
    scratch = os.urandom(max(profile["file_max"], profile["page_max"]))
    state = {"cycles": 0, "failures": 0, "command": 0, "page": 0, "file": 0, "payload_bytes": 0}
    establish_ms = []
    cycle_ms = []
    started = time.monotonic()
    deadline = started + duration

    while time.monotonic() < deadline:
        cycle_started = time.monotonic()
        up = threading.Event()
        link = RNS.Link(destination, established_callback=lambda _l: up.set())
        if not up.wait(10):
            state["failures"] += 1
            link.teardown()
            continue
        establish_ms.append((time.monotonic() - cycle_started) * 1000.0)

        band, size = roll_band(sizes, profile)
        moved = False
        if band == "command":
            receipt = RNS.Packet(link, scratch[:size]).send()
            waited = time.monotonic()
            while time.monotonic() - waited < 10:
                status = receipt.status if receipt else RNS.PacketReceipt.FAILED
                if status == RNS.PacketReceipt.DELIVERED:
                    moved = True
                    break
                if status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                    break
                time.sleep(0.0005)
        else:
            concluded = threading.Event()
            outcome = {}

            def callback(resource):
                outcome["status"] = resource.status
                concluded.set()

            RNS.Resource(scratch[:size], link, callback=callback)
            if concluded.wait(30):
                moved = outcome["status"] == RNS.Resource.COMPLETE

        if moved:
            state["payload_bytes"] += size
            state[band] += 1
        else:
            state["failures"] += 1
        link.teardown()
        time.sleep(0.002)
        if moved:
            state["cycles"] += 1
            cycle_ms.append((time.monotonic() - cycle_started) * 1000.0)
    elapsed_ms = int((time.monotonic() - started) * 1000)
    time.sleep(0.5)

    establish_ms = sorted(establish_ms)
    cycle_ms = sorted(cycle_ms)
    pct = lambda arr, p: (
        arr[min(round((len(arr) - 1) * p), len(arr) - 1)] if arr else float("nan")
    )
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT cycles={state['cycles']} failures={state['failures']} "
        f"commands={state['command']} pages={state['page']} files={state['file']} "
        f"payload_bytes={state['payload_bytes']} elapsed_ms={elapsed_ms} "
        f"cycles_per_sec={state['cycles'] / seconds:.1f} "
        f"establish_p50_ms={pct(establish_ms, 0.50):.0f} "
        f"establish_p99_ms={pct(establish_ms, 0.99):.0f} "
        f"cycle_p50_ms={pct(cycle_ms, 0.50):.0f} cycle_p99_ms={pct(cycle_ms, 0.99):.0f}",
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

    mechanism = manifest["profile"]["mechanism"]
    wire = manifest["profile"].get("wire", "tcp")
    topology = manifest["profile"].get("topology", "direct")
    if role == "relay":
        relay(manifest["name"], addr)
        return
    if role not in ("responder", "initiator"):
        sys.exit(usage)
    block, ready_addr = interface_block(wire, role, addr, topology)
    responders = {
        "link": respond_link,
        "resource": respond_resource,
        "request": respond_request,
        "churn": respond_churn,
    }
    initiators = {
        "link": initiate_link,
        "resource": initiate_resource,
        "request": initiate_request,
        "churn": initiate_churn,
    }
    if role == "responder":
        responders.get(mechanism, respond)(manifest["name"], block, ready_addr)
    else:
        initiators.get(mechanism, initiate)(
            manifest["name"], block, manifest["profile"], duration_ms / 1000.0
        )


if __name__ == "__main__":
    main()
