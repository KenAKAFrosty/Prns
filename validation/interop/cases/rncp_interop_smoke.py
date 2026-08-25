import os
import time
from pathlib import Path
from typing import Sequence

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    Peer,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    reference_utility,
    require_evidence,
    require_hex_output,
    require_listening_destination,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_ORACLE = ROOT / "validation/interop/peers/rns_rncp_oracle.py"
SUCCESS = (
    "PASS: Prnsd cp rejects partial publication, settles cancellation, and exchanges exact "
    "compressed, boundary, bulk, and completed multi-segment files with stock RNS rncp"
)
STOCK_FETCH_SAVE_DEFECT_MARKER = "Invalid save path"
STOCK_FETCH_WIRE_COMPLETE_MARKER = "Transfer complete"
STOCK_FETCH_SAVE_DEFECT_NOTICE = (
    "RNCP_STOCK_FETCH_SAVE_BLOCKED_BY_UPSTREAM "
    'reason="stock RNS 1.5.0 rncp -f normalizes its save path with os.path.abspath but guards '
    "it against a forward-slash prefix, so it can never save a fetched file on Windows; the "
    'wire transfer completed, so only the local byte comparison is skipped"'
)


def require_files_equal(expected: Path, actual: Path, failure: str) -> None:
    require_evidence(expected.read_bytes() == actual.read_bytes(), failure)


def conclude_stock_fetch(
    case: InteropCase,
    fetch: Peer,
    served: Path,
    fetched: Path,
    failure: str,
) -> None:
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        status = fetch.process.poll()
        if status is not None:
            if status != 0:
                raise InteropFailure(
                    FailureKind.PEER_EXITED,
                    f"{fetch.spec.name} exited with status {status}",
                )
            require_files_equal(served, fetched, failure)
            return
        if os.name == "nt" and STOCK_FETCH_SAVE_DEFECT_MARKER in case.read_log(fetch):
            case.wait_for(fetch, STOCK_FETCH_WIRE_COMPLETE_MARKER, 10)
            print(STOCK_FETCH_SAVE_DEFECT_NOTICE, flush=True)
            case.terminate(fetch)
            return
        time.sleep(0.1)
    raise InteropFailure(
        FailureKind.PEER_EXIT_TIMEOUT,
        f"timed out waiting for {fetch.spec.name} to conclude",
    )


def require_path_absent(path: Path, duration_seconds: float, failure: str) -> None:
    deadline = time.monotonic() + duration_seconds
    while time.monotonic() < deadline:
        if path.exists():
            raise InteropFailure(FailureKind.EVIDENCE_UNEXPECTED, failure)
        time.sleep(0.05)


def wait_for_no_staging(directory: Path, timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if not tuple(directory.glob(".rncp.*.staging")):
            return
        time.sleep(0.1)
    raise InteropFailure(
        FailureKind.EVIDENCE_UNEXPECTED,
        "cancelled stock RNS transfer left RNCP staging state",
    )


def start_listener(case: InteropCase, name: str, command: Sequence[str]) -> Peer:
    listener = case.start(PeerSpec(name, tuple(command), environment({})))
    case.wait_for(listener, "cp listening", 5)
    return listener


def start_public_listener(
    case: InteropCase,
    prnsd: Path,
    config: Path,
    identity: Path,
    receive_directory: Path,
    fetch_directory: Path,
) -> Peer:
    return start_listener(
        case,
        "Prnsd public RNCP listener",
        (
            str(prnsd),
            "cp",
            "--config",
            str(config),
            "-i",
            str(identity),
            "-l",
            "-n",
            "-F",
            "-j",
            str(fetch_directory),
            "-s",
            str(receive_directory),
        ),
    )


def start_authenticated_listener(
    case: InteropCase,
    prnsd: Path,
    config: Path,
    identity: Path,
    authorized_identity_hash: str,
    receive_directory: Path,
    fetch_directory: Path,
) -> Peer:
    return start_listener(
        case,
        "Prnsd authenticated RNCP listener",
        (
            str(prnsd),
            "cp",
            "--config",
            str(config),
            "-i",
            str(identity),
            "-l",
            "-F",
            "-a",
            authorized_identity_hash,
            "-s",
            str(receive_directory),
            "-j",
            str(fetch_directory),
        ),
    )


def wait_for_fresh_listener_request(case: InteropCase, command: Peer) -> None:
    case.wait_for(command, "Path to", 10)
    case.require_running(
        command,
        1,
        "stock rncp did not remain pending for the fresh listener announcement",
    )


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    stock_rncp = reference_utility("rncp")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with (
        PortLease() as bus_port,
        PortLease() as control_port,
        PortLease() as network_port,
        PortLease() as client_bus_port,
        PortLease() as client_control_port,
        InteropCase() as case,
    ):
        config = case.work / "config"
        client_config = case.work / "client-config"
        stock_identity = case.work / "stock.rid"
        client_identity = case.work / "client.rid"
        candidate_identity = case.work / "prns.rid"
        denied_candidate_identity = case.work / "denied-prns.rid"
        stock_receive = case.work / "stock-receive"
        candidate_receive = case.work / "prns-receive"
        stock_fetch = case.work / "stock-fetch"
        candidate_fetch = case.work / "prns-fetch"
        stock_fetched = case.work / "stock-fetched"
        candidate_fetched = case.work / "prns-fetched"
        authenticated_receive = case.work / "auth-receive"
        authenticated_fetched = case.work / "auth-fetched"
        for directory in (
            stock_receive,
            candidate_receive,
            stock_fetch,
            candidate_fetch,
            stock_fetched,
            candidate_fetched,
            authenticated_receive,
            authenticated_fetched,
        ):
            directory.mkdir()

        stock_destination = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "prepare",
                    str(config),
                    str(client_config),
                    str(bus_port.port),
                    str(control_port.port),
                    str(network_port.port),
                    str(client_bus_port.port),
                    str(client_control_port.port),
                    str(stock_identity),
                    str(client_identity),
                ),
                "stock RNS did not prepare the rncp configuration",
            ),
            16,
            "stock RNS did not report a valid rncp destination",
        )
        run_checked(
            (str(python), str(STOCK_ORACLE), "prepare-fixtures", str(case.work)),
            "stock RNS did not prepare RNCP transfer fixtures",
        )
        run_checked(
            (str(prnsd), "id", "-g", str(candidate_identity)),
            "Prnsd did not prepare its RNCP sender identity",
        )
        candidate_identity_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "identity-hash",
                    str(candidate_identity),
                ),
                "stock RNS could not read the Prns RNCP sender identity",
            ),
            16,
            "stock RNS did not report a valid Prns RNCP sender identity hash",
        )
        run_checked(
            (str(prnsd), "id", "-g", str(denied_candidate_identity)),
            "Prnsd did not prepare the unlisted RNCP sender identity",
        )
        denied_candidate_identity_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "identity-hash",
                    str(denied_candidate_identity),
                ),
                "stock RNS could not read the unlisted Prns RNCP identity",
            ),
            16,
            "stock RNS did not report a valid unlisted Prns RNCP identity hash",
        )

        bus_port.release()
        control_port.release()
        network_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS RNCP server",
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "serve",
                    str(config),
                    str(stock_identity),
                    str(candidate_identity),
                    str(stock_receive),
                    str(stock_fetch),
                ),
                environment({}),
            )
        )
        case.wait_for(server, f"RNCP_SERVER_READY {stock_destination}", 10)
        client_bus_port.release()
        client_control_port.release()
        client = case.start_reference_rns(
            PeerSpec(
                "stock RNS RNCP client instance",
                (str(python), str(STOCK_ORACLE), "hold", str(client_config)),
                environment({}),
            )
        )
        case.wait_for(client, "RNCP_CLIENT_READY", 10)

        interrupted_source = case.work / "interrupt-prns.bin"
        interrupted = case.start(
            PeerSpec(
                "interrupted Prns RNCP sender",
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(candidate_identity),
                    "-C",
                    str(interrupted_source),
                    stock_destination,
                ),
                environment({}),
            )
        )
        case.wait_for(interrupted, "Transferring file", 10)
        case.wait_for(server, "RNCP_RESOURCE_ACTIVE progress=", 10)
        interrupted_status = case.terminate(interrupted)
        if interrupted_status == 0:
            raise InteropFailure(
                FailureKind.COMMAND_FAILED,
                "interrupted Prns RNCP sender reported success",
            )
        require_path_absent(
            stock_receive / interrupted_source.name,
            1,
            "stock rncp published interrupted Prns bytes",
        )

        candidate_send = case.work / "prns-send.bin"
        run_checked(
            (
                str(prnsd),
                "cp",
                "--config",
                str(config),
                "-i",
                str(candidate_identity),
                "-S",
                "-P",
                str(candidate_send),
                stock_destination,
            ),
            "Prnsd could not send a Resource to stock rncp",
        )
        received_candidate_send = stock_receive / candidate_send.name
        case.wait_for_path(server, received_candidate_send, 10)
        require_files_equal(
            candidate_send,
            received_candidate_send,
            "stock rncp received different Prns bytes",
        )
        case.wait_for(
            server,
            "RNCP_SINGLE_SEGMENT_RECEIVED name=prns-send.bin segments=1",
            10,
        )
        case.wait_for(server, f"RNCP_PRNS_IDENTIFIED {candidate_identity_hash}", 10)

        denied_candidate_source = case.work / "denied-prns.bin"
        denied_candidate_source.write_bytes(b"denied-prns-to-stock\n" * 1024)
        denied_candidate = case.start(
            PeerSpec(
                "unlisted Prns RNCP sender",
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(denied_candidate_identity),
                    "-w",
                    "5",
                    str(denied_candidate_source),
                    stock_destination,
                ),
                environment({}),
            )
        )
        denied_candidate_status = case.wait_for_status(denied_candidate, 15)
        if denied_candidate_status == 0:
            raise InteropFailure(
                FailureKind.COMMAND_FAILED,
                "stock RNS accepted an unlisted Prns RNCP sender",
            )
        case.wait_for(
            server,
            f"RNCP_PRNS_UNAUTHORIZED {denied_candidate_identity_hash}",
            10,
        )
        require_path_absent(
            stock_receive / denied_candidate_source.name,
            1,
            "stock RNS published bytes from an unlisted Prns RNCP sender",
        )

        candidate_compressed = case.work / "prns-compressed.bin"
        run_checked(
            (
                str(prnsd),
                "cp",
                "--config",
                str(config),
                "-i",
                str(candidate_identity),
                "-S",
                str(candidate_compressed),
                stock_destination,
            ),
            "Prnsd could not send a compressed Resource to stock rncp",
        )
        received_candidate_compressed = stock_receive / candidate_compressed.name
        case.wait_for_path(server, received_candidate_compressed, 10)
        require_files_equal(
            candidate_compressed,
            received_candidate_compressed,
            "stock rncp reconstructed different compressed Prns bytes",
        )
        require_output_marker(
            case.read_log(server),
            "RNCP_COMPRESSED_RECEIVED name=prns-compressed.bin",
            "stock RNS did not observe compressed transport from Prns",
        )

        candidate_segmented = case.work / "prns-segmented.bin"
        run_checked(
            (
                str(prnsd),
                "cp",
                "--config",
                str(config),
                "-i",
                str(candidate_identity),
                "-S",
                "-P",
                "-C",
                str(candidate_segmented),
                stock_destination,
            ),
            "Prnsd could not send a multi-segment Resource to stock rncp",
        )
        received_candidate_segmented = stock_receive / candidate_segmented.name
        case.wait_for_path(server, received_candidate_segmented, 20)
        require_files_equal(
            candidate_segmented,
            received_candidate_segmented,
            "stock rncp received different completed multi-segment Prns bytes",
        )
        require_output_marker(
            case.read_log(server),
            "RNCP_SEGMENTED_RECEIVED name=prns-segmented.bin segments=",
            "stock RNS did not observe multiple segments from Prns",
        )

        candidate_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(candidate_identity),
                    "-p",
                ),
                "Prnsd did not derive its RNCP listener destination",
            ),
            "Prnsd did not report its RNCP listener destination",
        )
        recovery_files = (
            case.work / "boundary-1.bin",
            case.work / "boundary-464.bin",
            case.work / "boundary-465.bin",
            case.work / "stock-send.bin",
            case.work / "stock-compressed.bin",
            case.work / "stock-segmented.bin",
        )
        cancel_source = case.work / "cancel-stock.bin"
        cancellation = case.start_reference_rns(
            PeerSpec(
                "stock RNS RNCP cancellation sender",
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "cancel-send",
                    str(client_config),
                    str(client_identity),
                    candidate_destination,
                    str(cancel_source),
                    *(str(path) for path in recovery_files),
                ),
                environment({}),
            )
        )
        case.wait_for(cancellation, "RNCP_CANCEL_PATH_REQUESTED", 10)
        case.require_running(
            cancellation,
            1,
            "stock RNS cancellation sender did not wait for the fresh Prns listener",
        )
        listener = start_public_listener(
            case,
            prnsd,
            config,
            candidate_identity,
            candidate_receive,
            candidate_fetch,
        )
        case.wait_for_exit(cancellation, 60)
        cancellation_result = case.read_log(cancellation)
        require_output_marker(
            cancellation_result,
            "RNCP_CANCEL_OK",
            "stock RNS Resource cancellation did not settle",
        )
        require_output_marker(
            cancellation_result,
            "segmented_recoveries=1",
            "stock RNS did not complete a multi-segment recovery into Prns",
        )
        require_output_marker(
            cancellation_result,
            "compressed_recoveries=1",
            "stock RNS did not send a compressed Resource into Prns",
        )
        require_output_marker(
            cancellation_result,
            "single_segment_recoveries=5",
            "stock RNS did not send single-segment Resources into Prns",
        )
        wait_for_no_staging(candidate_receive, 10)
        if (candidate_receive / cancel_source.name).exists():
            raise InteropFailure(
                FailureKind.EVIDENCE_UNEXPECTED,
                "Prnsd published cancelled stock RNS bytes",
            )
        for recovery_file in recovery_files:
            received_recovery = candidate_receive / recovery_file.name
            case.wait_for_path(listener, received_recovery, 10)
            require_files_equal(
                recovery_file,
                received_recovery,
                f"RNCP recovery file {recovery_file.name} did not round-trip",
            )

        run_checked(
            (
                str(prnsd),
                "cp",
                "--config",
                str(config),
                "-i",
                str(candidate_identity),
                "-S",
                "-P",
                "-f",
                "-s",
                str(candidate_fetched),
                "stock.txt",
                stock_destination,
            ),
            "Prnsd could not fetch a file from stock rncp",
        )
        require_files_equal(
            stock_fetch / "stock.txt",
            candidate_fetched / "stock.txt",
            "Prnsd fetched different stock rncp bytes",
        )
        case.wait_for(server, f"RNCP_PRNS_FETCH_AUTHORIZED {candidate_identity_hash}", 10)
        case.stop(listener)

        public_fetch_identity = case.work / "public-fetch.rid"
        public_fetch_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(public_fetch_identity),
                    "-p",
                ),
                "Prnsd did not derive the public-fetch listener destination",
            ),
            "Prnsd did not report the public-fetch listener destination",
        )
        public_fetch = case.start(
            PeerSpec(
                "stock rncp public fetch",
                (
                    str(stock_rncp),
                    "--config",
                    str(client_config),
                    "-i",
                    str(client_identity),
                    "-f",
                    "-s",
                    str(stock_fetched),
                    "prns.txt",
                    public_fetch_destination,
                ),
                environment({}),
            )
        )
        wait_for_fresh_listener_request(case, public_fetch)
        listener = start_public_listener(
            case,
            prnsd,
            config,
            public_fetch_identity,
            candidate_receive,
            candidate_fetch,
        )
        conclude_stock_fetch(
            case,
            public_fetch,
            candidate_fetch / "prns.txt",
            stock_fetched / "prns.txt",
            "stock rncp fetched different Prns bytes",
        )
        case.stop(listener)

        client_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "identity-hash",
                    str(client_identity),
                ),
                "stock RNS could not read the authorized RNCP client identity",
            ),
            16,
            "stock RNS did not report a valid authorized RNCP client hash",
        )
        authenticated_send_identity = case.work / "auth-send.rid"
        authenticated_send_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(authenticated_send_identity),
                    "-p",
                ),
                "Prnsd did not derive the authenticated-send listener destination",
            ),
            "Prnsd did not report the authenticated-send listener destination",
        )
        authenticated_send = case.start(
            PeerSpec(
                "authorized stock rncp sender",
                (
                    str(stock_rncp),
                    "--config",
                    str(client_config),
                    "-i",
                    str(client_identity),
                    str(case.work / "stock-send.bin"),
                    authenticated_send_destination,
                ),
                environment({}),
            )
        )
        wait_for_fresh_listener_request(case, authenticated_send)
        listener = start_authenticated_listener(
            case,
            prnsd,
            config,
            authenticated_send_identity,
            client_hash,
            authenticated_receive,
            candidate_fetch,
        )
        case.wait_for_exit(authenticated_send, 60)
        authenticated_received = authenticated_receive / "stock-send.bin"
        case.wait_for_path(listener, authenticated_received, 10)
        require_files_equal(
            case.work / "stock-send.bin",
            authenticated_received,
            "Prnsd rejected or changed the authorized stock rncp bytes",
        )
        case.stop(listener)

        authenticated_fetch_identity = case.work / "auth-fetch.rid"
        authenticated_fetch_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(authenticated_fetch_identity),
                    "-p",
                ),
                "Prnsd did not derive the authenticated-fetch listener destination",
            ),
            "Prnsd did not report the authenticated-fetch listener destination",
        )
        authenticated_fetch = case.start(
            PeerSpec(
                "authorized stock rncp fetch",
                (
                    str(stock_rncp),
                    "--config",
                    str(client_config),
                    "-i",
                    str(client_identity),
                    "-f",
                    "-s",
                    str(authenticated_fetched),
                    "prns.txt",
                    authenticated_fetch_destination,
                ),
                environment({}),
            )
        )
        wait_for_fresh_listener_request(case, authenticated_fetch)
        listener = start_authenticated_listener(
            case,
            prnsd,
            config,
            authenticated_fetch_identity,
            client_hash,
            authenticated_receive,
            candidate_fetch,
        )
        conclude_stock_fetch(
            case,
            authenticated_fetch,
            candidate_fetch / "prns.txt",
            authenticated_fetched / "prns.txt",
            "Prnsd rejected or changed the authorized stock rncp fetch",
        )
        case.stop(listener)

        denied_identity = case.work / "denied.rid"
        denied_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "cp",
                    "--config",
                    str(config),
                    "-i",
                    str(denied_identity),
                    "-p",
                ),
                "Prnsd did not derive the denied listener destination",
            ),
            "Prnsd did not report the denied listener destination",
        )
        denied = case.start(
            PeerSpec(
                "unlisted stock rncp sender",
                (
                    str(stock_rncp),
                    "--config",
                    str(client_config),
                    "-i",
                    str(stock_identity),
                    "-w",
                    "5",
                    str(candidate_send),
                    denied_destination,
                ),
                environment({}),
            )
        )
        wait_for_fresh_listener_request(case, denied)
        listener = start_authenticated_listener(
            case,
            prnsd,
            config,
            denied_identity,
            client_hash,
            authenticated_receive,
            candidate_fetch,
        )
        denied_status = case.wait_for_status(denied, 15)
        if denied_status == 0:
            raise InteropFailure(
                FailureKind.COMMAND_FAILED,
                "Prnsd accepted an unlisted stock rncp sender",
            )
        require_path_absent(
            authenticated_receive / candidate_send.name,
            1,
            "Prnsd published bytes from an unlisted stock rncp sender",
        )
        case.stop(listener)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
