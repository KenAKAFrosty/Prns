#!/usr/bin/env python3
"""The pinned RNS reference participant for live scenarios - the same contract as
`participant_node`: `participant_node.py <manifest.json> <role> <addr> [duration-ms]`,
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
from collections import deque

import RNS
from workload_vectors import DEFAULT_SIZE_SEED, SizeSequence, deterministic_payload

ANNOUNCE_EVERY = 0.5
INITIATOR_COUNT = 1
DRAIN_GRACE = 5.0
QUIET_AFTER_TRAFFIC = 1.5
REQUEST_PATH = "/bench/query"
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


def await_measurement_start():
    print("MEASURE_READY", flush=True)
    command = sys.stdin.readline().strip()
    if command != "START":
        sys.exit(f"expected START measurement command, received {command!r}")


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


def interface_block(wire, role, addr, fixed_mtu=None):
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
        return (
            "  [[Bench TCP Server]]\n"
            "    type = TCPServerInterface\n"
            "    enabled = True\n"
            "    listen_ip = 127.0.0.1\n"
            f"    listen_port = {port}\n"
            + mtu_line
        ), f"127.0.0.1:{port}"
    host, port = addr.rsplit(":", 1)
    mtu_line = f"    fixed_mtu = {fixed_mtu}\n" if fixed_mtu else ""
    return (
        "  [[Bench TCP Client]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = True\n"
        f"    target_host = {host}\n"
        f"    target_port = {port}\n"
        + mtu_line
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
    scratch = scenario_payload(profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
    state = {"sent": 0, "delivered": 0, "timeouts": 0, "delivered_bytes": 0}
    rtts = []
    await_measurement_start()
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
    streak_limit = max(profile["window"] * 8, 64)
    failure_streak = 0
    died = False
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
                failure_streak = 0
                if not died and time.monotonic() < deadline:
                    still.append(send_one())
            elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                state["timeouts"] += 1
                settled += 1
                failure_streak += 1
                if not died and failure_streak >= streak_limit:
                    died = True
                    print(f"DIED failure_streak={failure_streak}", file=sys.stderr, flush=True)
                if not died and time.monotonic() < deadline:
                    still.append(send_one())
            else:
                still.append((receipt, size))
        outstanding = still
        if settled == 0:
            time.sleep(0.0005)
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
        with links_lock:
            links["up"] += 1
        link.set_packet_callback(on_packet)
        link.set_link_closed_callback(on_closed)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
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
    scratch = scenario_payload(profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
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

    # Receipts are POLLED, not callback-driven: on localhost the proof can conclude a
    # receipt before a delivery callback could even be registered, and the reference
    # never fires callbacks retroactively.
    def send_one():
        state["sent"] += 1
        size = sizes.next_len()
        state["sent_bytes"] += size
        return RNS.Packet(link, scratch[:size]).send(), size

    outstanding = [send_one() for _ in range(profile["window"])]
    streak_limit = max(profile["window"] * 8, 64)
    failure_streak = 0
    died = False
    while outstanding and time.monotonic() < drain_deadline:
        still = []
        settled = 0
        for receipt, size in outstanding:
            status = receipt.status if receipt else RNS.PacketReceipt.FAILED
            if status == RNS.PacketReceipt.DELIVERED:
                state["receipt_proved"] += 1
                rtts.append(receipt.get_rtt() * 1000.0)
                settled += 1
                failure_streak = 0
                if not died and time.monotonic() < deadline:
                    still.append(send_one())
            elif status in (RNS.PacketReceipt.FAILED, RNS.PacketReceipt.CULLED):
                state["receipt_unproved"] += 1
                settled += 1
                failure_streak += 1
                if not died and failure_streak >= streak_limit:
                    died = True
                    print(f"DIED failure_streak={failure_streak}", file=sys.stderr, flush=True)
                if not died and time.monotonic() < deadline:
                    still.append(send_one())
            else:
                still.append((receipt, size))
        outstanding = still
        if settled == 0:
            time.sleep(0.0005)
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


def respond_resource(name, block, ready_addr, _profile):
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

    links = {"up": 0, "closed": 0}
    links_lock = threading.Lock()

    def on_closed(_link):
        with links_lock:
            links["closed"] += 1
        if links["closed"] >= INITIATOR_COUNT:
            done.set()

    def on_link(link):
        with links_lock:
            links["up"] += 1
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(on_concluded)
        link.set_link_closed_callback(on_closed)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
    while not done.is_set():
        if links["up"] < INITIATOR_COUNT:
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

    sizes = sizes_from(profile, "payload_min", "payload_max", "payload_len")
    scratch = scenario_payload(profile, max(profile.get("payload_max", 0), profile.get("payload_len", 0)))
    state = {"sent": 0, "settled": 0, "failures": 0, "settled_bytes": 0}
    transfer_ms = []
    await_measurement_start()
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
        RNS.Resource(scratch[:size], link, auto_compress=auto_compress_from(profile), callback=callback)
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
    print("MEASURE_DONE", flush=True)
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
        with links_lock:
            links["up"] += 1
        link.set_link_closed_callback(on_closed)

    destination.set_link_established_callback(on_link)
    print(f"READY role=responder addr={ready_addr}", flush=True)
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
    lock = threading.Lock()
    settled = threading.Event()
    available_links = deque(links)
    started = None
    deadline = None

    def on_response(receipt):
        with lock:
            state["delivered"] += 1
            state["in_flight"] -= 1
            available_links.append(receipt.link)
            state["response_bytes"] += len(receipt.response or b"")
            rtts.append((time.monotonic() - receipt.sent_at_wall) * 1000.0)
        settled.set()

    def on_failed(receipt):
        with lock:
            state["timeouts"] += 1
            state["in_flight"] -= 1
            available_links.append(receipt.link)
        settled.set()

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

    await_measurement_start()
    started = time.monotonic()
    deadline = started + duration
    with lock:
        for _ in range(profile["window"]):
            send_one(available_links.popleft())
    drain_deadline = deadline + DRAIN_GRACE
    while time.monotonic() < drain_deadline:
        with lock:
            in_flight = state["in_flight"]
            if in_flight < profile["window"] and available_links and time.monotonic() < deadline:
                send_one(available_links.popleft())
                continue
            if in_flight == 0:
                break
        settled.wait(0.05)
        settled.clear()
    elapsed_ms = int((time.monotonic() - started) * 1000)
    print("MEASURE_DONE", flush=True)
    for link in links:
        link.teardown()
    time.sleep(0.5)

    rtts = sorted(rtts)
    pct = lambda p: rtts[min(round((len(rtts) - 1) * p), len(rtts) - 1)] if rtts else float("nan")
    seconds = max(elapsed_ms / 1000.0, 1e-9)
    print(
        f"RESULT sent={state['sent']} delivered={state['delivered']} "
        f"timeouts={state['timeouts']} raced=0 "
        f"request_bytes={state['request_bytes']} "
        f"response_bytes={state['response_bytes']} "
        f"expected_response_bytes={state['expected_response_bytes']} elapsed_ms={elapsed_ms} "
        f"requests_per_sec={state['delivered'] / seconds:.1f} "
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
    if role not in ("responder", "initiator"):
        sys.exit(usage)
    block, ready_addr = interface_block(
        wire, role, addr, manifest["profile"].get("link_mtu")
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
