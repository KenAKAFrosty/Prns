import hashlib
import os
import pathlib
import shutil
import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum

LISTENER_PRIVATE = bytes([0x31]) * 32 + bytes([0x32]) * 32
CLIENT_PRIVATE = bytes([0x41]) * 32 + bytes([0x42]) * 32
RECEIVER_RECEIPT_DELIVERY_SECONDS = 1.0


def prepare(
    config_dir,
    client_config_dir,
    bus_port,
    control_port,
    network_port,
    client_bus_port,
    client_control_port,
    listener_path,
    client_path,
):
    config_dir = pathlib.Path(config_dir)
    client_config_dir = pathlib.Path(client_config_dir)
    config_dir.mkdir(parents=True, exist_ok=True)
    client_config_dir.mkdir(parents=True, exist_ok=True)
    config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = Yes\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[RNCP Network]]\n"
        "type = TCPServerInterface\n"
        "enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {network_port}\n",
        encoding="utf-8",
    )
    client_config_dir.joinpath("config").write_text(
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = Yes\n"
        "shared_instance_type = TCP\n"
        f"shared_instance_port = {client_bus_port}\n"
        f"instance_control_port = {client_control_port}\n"
        "[logging]\n"
        "loglevel = 2\n"
        "[interfaces]\n"
        "[[RNCP Client]]\n"
        "type = TCPClientInterface\n"
        "enabled = Yes\n"
        "target_host = 127.0.0.1\n"
        f"target_port = {network_port}\n",
        encoding="utf-8",
    )
    listener = RNS.Identity.from_bytes(LISTENER_PRIVATE)
    listener.to_file(listener_path)
    RNS.Identity.from_bytes(CLIENT_PRIVATE).to_file(client_path)
    print(RNS.Destination.hash(listener, "rncp", "receive").hex())


def hold(config_dir):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    print("RNCP_CLIENT_READY", flush=True)
    while True:
        time.sleep(0.25)


def serve(config_dir, listener_path, expected_client_path, save_path, fetch_path):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    listener = RNS.Identity.from_file(listener_path)
    expected_client = RNS.Identity.from_file(expected_client_path)
    if expected_client is None:
        raise RuntimeError("expected Prns RNCP identity did not load")
    save_path = pathlib.Path(save_path).resolve()
    fetch_path = pathlib.Path(fetch_path).resolve()
    destination = RNS.Destination(
        listener,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "rncp",
        "receive",
    )

    def concluded(resource):
        if resource.status != RNS.Resource.COMPLETE or resource.metadata is None:
            return
        name = os.path.basename(resource.metadata["name"].decode("utf-8"))
        segments = resource.get_segments()
        if name == "prns-send.bin" and segments != 1:
            raise RuntimeError(f"Prns single-segment transfer used {segments} segments")
        if name == "prns-segmented.bin" and segments <= 1:
            raise RuntimeError("Prns segmented transfer completed as a single segment")
        if name == "prns-compressed.bin":
            if not resource.is_compressed():
                raise RuntimeError("Prns compressible Resource arrived uncompressed")
            if resource.get_transfer_size() >= resource.get_data_size():
                raise RuntimeError("Prns compressed Resource did not reduce transport bytes")
        target = save_path.joinpath(name)
        counter = 0
        while target.exists():
            counter += 1
            target = save_path.joinpath(f"{name}.{counter}")
        resource.data.close()
        shutil.move(resource.data.name, target)
        if name == "prns-send.bin":
            print(f"RNCP_SINGLE_SEGMENT_RECEIVED name={name} segments={segments}", flush=True)
        if name == "prns-segmented.bin":
            print(f"RNCP_SEGMENTED_RECEIVED name={name} segments={segments}", flush=True)
        if name == "prns-compressed.bin":
            print(
                f"RNCP_COMPRESSED_RECEIVED name={name} "
                f"transport={resource.get_transfer_size()} data={resource.get_data_size()}",
                flush=True,
            )

    active_resources = []
    progress_reported = set()

    def identified(link, identity):
        if identity.hash == expected_client.hash:
            print(f"RNCP_PRNS_IDENTIFIED {identity.hash.hex()}", flush=True)

    def authorize(resource):
        identity = resource.link.get_remote_identity()
        if identity is None:
            print("RNCP_PRNS_UNAUTHORIZED anonymous", flush=True)
            return False
        if identity.hash != expected_client.hash:
            print(f"RNCP_PRNS_UNAUTHORIZED {identity.hash.hex()}", flush=True)
            return False
        return True

    def started(resource):
        active_resources.append(resource)

    def established(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_APP)
        link.set_remote_identified_callback(identified)
        link.set_resource_callback(authorize)
        link.set_resource_started_callback(started)
        link.set_resource_concluded_callback(concluded)

    def fetch(path, data, request_id, link_id, remote_identity, requested_at):
        if remote_identity is None or remote_identity.hash != expected_client.hash:
            return False
        candidate = fetch_path.joinpath(str(data).lstrip("/")).resolve()
        if fetch_path not in candidate.parents or not candidate.is_file():
            return False
        for active in RNS.Transport.active_links:
            if active.link_id == link_id:
                metadata = {"name": candidate.name.encode("utf-8")}
                RNS.Resource(open(candidate, "rb"), active, metadata=metadata)
                print(f"RNCP_PRNS_FETCH_AUTHORIZED {remote_identity.hash.hex()}", flush=True)
                return True
        return None

    destination.set_link_established_callback(established)
    destination.register_request_handler(
        "fetch_file",
        response_generator=fetch,
        allow=RNS.Destination.ALLOW_ALL,
    )
    destination.announce()
    print(f"RNCP_SERVER_READY {destination.hash.hex()}", flush=True)
    while True:
        for resource in active_resources:
            marker = bytes(resource.hash)
            if marker not in progress_reported and resource.get_progress() > 0:
                progress_reported.add(marker)
                print(f"RNCP_RESOURCE_ACTIVE progress={resource.get_progress():.6f}", flush=True)
        time.sleep(0.25)


def identity_hash(path):
    print(RNS.Identity.from_file(path).hash.hex())


def prepare_fixtures(work_path):
    work = pathlib.Path(work_path)
    work.joinpath("prns-send.bin").write_bytes(b"prns-to-stock\n" * 12000)
    work.joinpath("stock-send.bin").write_bytes(b"stock-to-prns\n" * 12000)
    work.joinpath("prns-compressed.bin").write_bytes(b"prns-compressed-resource\n" * 12000)
    work.joinpath("stock-compressed.bin").write_bytes(b"stock-compressed-resource\n" * 12000)
    work.joinpath("stock-fetch/stock.txt").write_bytes(b"served-by-stock\n" * 12000)
    work.joinpath("prns-fetch/prns.txt").write_bytes(b"served-by-prns\n" * 12000)
    work.joinpath("interrupt-prns.bin").write_bytes(os.urandom(32 * 1024 * 1024))
    work.joinpath("cancel-stock.bin").write_bytes(os.urandom(32 * 1024 * 1024))
    segment_crossing_size = RNS.Resource.MAX_EFFICIENT_SIZE + 4096
    for name, seed in (("prns-segmented.bin", b"prns"), ("stock-segmented.bin", b"stock")):
        blocks = []
        generated = 0
        counter = 0
        while generated < segment_crossing_size:
            block = hashlib.sha256(seed + counter.to_bytes(8, "big")).digest()
            blocks.append(block)
            generated += len(block)
            counter += 1
        work.joinpath(name).write_bytes(b"".join(blocks)[:segment_crossing_size])
    for size in (1, 464, 465):
        work.joinpath(f"boundary-{size}.bin").write_bytes(
            bytes((index * 37) & 0xFF for index in range(size))
        )


def wait_for(predicate, timeout, failure):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise RuntimeError(failure)


def cancel_send(config_dir, identity_path, destination_hash, source_path, *recovery_paths):
    start_reference_reticulum(configdir=config_dir, loglevel=RNS.LOG_ERROR)
    destination_hash = bytes.fromhex(destination_hash)
    if not RNS.Transport.has_path(destination_hash):
        RNS.Transport.request_path(destination_hash)
        print("RNCP_CANCEL_PATH_REQUESTED", flush=True)
    wait_for(
        lambda: RNS.Transport.has_path(destination_hash),
        10,
        "cancel destination path was not learned",
    )
    remote_identity = RNS.Identity.recall(destination_hash)
    if remote_identity is None:
        raise RuntimeError("cancel destination identity was not recalled")
    destination = RNS.Destination(
        remote_identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        "rncp",
        "receive",
    )
    link = RNS.Link(destination)
    wait_for(lambda: link.status == RNS.Link.ACTIVE, 10, "cancel link did not activate")
    local_identity = RNS.Identity.from_file(identity_path)
    if local_identity is None:
        raise RuntimeError("cancel sender identity did not load")
    link.identify(local_identity)
    source_path = pathlib.Path(source_path)
    source = open(source_path, "rb")
    resource = RNS.Resource(
        source,
        link,
        metadata={"name": source_path.name.encode("utf-8")},
        auto_compress=False,
    )
    wait_for(
        lambda: resource.get_progress() > 0 or resource.status >= RNS.Resource.COMPLETE,
        15,
        "cancel transfer did not start",
    )
    if resource.status >= RNS.Resource.COMPLETE:
        raise RuntimeError("cancel transfer completed before cancellation")
    resource.cancel()
    wait_for(
        lambda: resource.status == RNS.Resource.FAILED,
        3,
        "cancel transfer did not settle as failed",
    )
    link.teardown()
    source.close()
    time.sleep(0.25)
    segmented_recoveries = 0
    compressed_recoveries = 0
    single_segment_recoveries = 0
    for recovery_path in map(pathlib.Path, recovery_paths):
        recovery_link = RNS.Link(destination)
        wait_for(
            lambda: recovery_link.status == RNS.Link.ACTIVE,
            10,
            f"recovery link for {recovery_path.name} did not activate",
        )
        recovery_link.identify(local_identity)
        recovery_source = open(recovery_path, "rb")
        recovery = RNS.Resource(
            recovery_source,
            recovery_link,
            metadata={"name": recovery_path.name.encode("utf-8")},
            auto_compress=recovery_path.name == "stock-compressed.bin",
        )
        if recovery_path.name == "stock-compressed.bin":
            if not recovery.is_compressed():
                raise RuntimeError("stock compressible Resource was not compressed")
            if recovery.get_transfer_size() >= recovery.get_data_size():
                raise RuntimeError("stock compressed Resource did not reduce transport bytes")
            compressed_recoveries += 1
        if recovery_path.stat().st_size <= RNS.Resource.MAX_EFFICIENT_SIZE:
            if recovery.get_segments() != 1:
                raise RuntimeError(
                    f"recovery transfer {recovery_path.name} used {recovery.get_segments()} segments"
                )
            single_segment_recoveries += 1
        wait_for(
            lambda: recovery.status >= RNS.Resource.COMPLETE,
            30,
            f"recovery transfer {recovery_path.name} did not conclude",
        )
        if recovery.status != RNS.Resource.COMPLETE:
            raise RuntimeError(
                f"recovery transfer {recovery_path.name} failed with status {recovery.status}"
            )
        if recovery_path.stat().st_size > RNS.Resource.MAX_EFFICIENT_SIZE:
            if recovery.get_segments() <= 1:
                raise RuntimeError(
                    f"recovery transfer {recovery_path.name} did not cross a segment boundary"
                )
            segmented_recoveries += 1
        time.sleep(RECEIVER_RECEIPT_DELIVERY_SECONDS)
        recovery_source.close()
        recovery_link.teardown()
    print(
        f"RNCP_CANCEL_OK progress={resource.get_progress():.6f} "
        f"recovery_files={len(recovery_paths)} segmented_recoveries={segmented_recoveries} "
        f"compressed_recoveries={compressed_recoveries} "
        f"single_segment_recoveries={single_segment_recoveries}"
    )


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(*sys.argv[2:])
    elif sys.argv[1] == "serve":
        serve(*sys.argv[2:])
    elif sys.argv[1] == "hold":
        hold(*sys.argv[2:])
    elif sys.argv[1] == "identity-hash":
        identity_hash(*sys.argv[2:])
    elif sys.argv[1] == "prepare-fixtures":
        prepare_fixtures(*sys.argv[2:])
    elif sys.argv[1] == "cancel-send":
        cancel_send(*sys.argv[2:])
    else:
        raise RuntimeError("unknown command")
