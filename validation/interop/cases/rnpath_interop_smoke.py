import json
import re
import time
from pathlib import Path

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    require_evidence,
    require_hex_output,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_SERVER = ROOT / "validation/interop/peers/rns_rnpath_server.py"
BLACKHOLE_HASH = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
RATE_HASH = "33333333333333333333333333333333"
PATH_DROP_TIMEOUT_SECONDS = 30.0
SUCCESS = "PASS: Prnsd path queried and mutated the stock RNS utility surfaces"


def load_rows(output: str, failure: str) -> list[dict[str, object]]:
    try:
        rows = json.loads(output)
    except json.JSONDecodeError as error:
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure) from error
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure)
    return rows


def require_row(
    rows: list[dict[str, object]],
    expected_hash: str,
    failure: str,
) -> dict[str, object]:
    for row in rows:
        if row.get("hash") == expected_hash:
            return row
    raise InteropFailure(FailureKind.EVIDENCE_MISSING, failure)


def drop_path_when_present(prnsd: Path, config: Path, destination_hash: str) -> None:
    """Drop a route the stock instance may still be installing or removing.

    A shared instance does not guarantee that a mutation is visible to the next query, so a
    route read from the path table can already be gone by the time the drop is issued. Retry
    the read and the drop together, and re-raise once the deadline passes so a route that is
    genuinely undroppable still fails the case.
    """
    deadline = time.monotonic() + PATH_DROP_TIMEOUT_SECONDS
    while True:
        wait_for_path_table(prnsd, config, destination_hash)
        try:
            run_checked(
                (str(prnsd), "path", "--config", str(config), "-d", destination_hash),
                "Prnsd could not drop a stock RNS path",
            )
            return
        except InteropFailure:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


def wait_for_path_table(prnsd: Path, config: Path, destination_hash: str) -> str:
    deadline = time.monotonic() + 10
    latest_output = ""
    while time.monotonic() < deadline:
        try:
            latest_output = run_checked(
                (str(prnsd), "path", "--config", str(config), "-t", "-j"),
                "Prnsd could not query the stock RNS local path table",
            )
        except InteropFailure as error:
            latest_output = error.detail
        if destination_hash in latest_output:
            return latest_output
        time.sleep(0.1)
    rendered = latest_output.rstrip()
    detail = "stock RNS peer route did not appear in the Prns path table"
    if rendered:
        detail = f"{detail}\n{rendered}"
    raise InteropFailure(FailureKind.MARKER_TIMEOUT, detail)


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with (
        PortLease() as bus_port,
        PortLease() as control_port,
        PortLease() as network_port,
        InteropCase() as case,
    ):
        config = case.work / "config"
        peer_config = case.work / "peer-config"
        management_identity = case.work / "management_identity"
        run_checked(
            (
                str(python),
                str(STOCK_SERVER),
                "prepare",
                str(config),
                str(peer_config),
                str(bus_port.port),
                str(control_port.port),
                str(network_port.port),
                str(management_identity),
            ),
            "stock RNS did not prepare the rnpath configuration",
        )
        bus_port.release()
        control_port.release()
        network_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS rnpath server",
                (str(python), str(STOCK_SERVER), "serve", str(config)),
                environment({}),
            )
        )
        case.wait_for(server, "RNPATH_SERVER_READY", 10)
        peer = case.start_reference_rns(
            PeerSpec(
                "stock RNS rnpath peer",
                (str(python), str(STOCK_SERVER), "peer", str(peer_config)),
                environment({}),
            )
        )
        case.wait_for(peer, "RNPATH_PEER_READY ", 10)
        peer_match = re.search(
            r"^RNPATH_PEER_READY ([0-9a-f]{32})$",
            case.read_log(peer),
            re.MULTILINE,
        )
        if peer_match is None:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "stock RNS rnpath peer did not report a valid destination hash",
            )
        peer_hash = peer_match.group(1)

        path_result = wait_for_path_table(prnsd, config, peer_hash)
        path_row = require_row(
            load_rows(path_result, "Prnsd did not return a valid local path table"),
            peer_hash,
            "Prnsd local path table omitted the stock RNS peer",
        )
        require_evidence(
            isinstance(path_row.get("hops"), int)
            and path_row["hops"] >= 1
            and bool(path_row.get("interface")),
            "Prnsd did not decode the stock RNS peer route",
        )
        via_hash = require_hex_output(
            str(path_row.get("via", "")),
            16,
            "Prnsd did not report a valid next hop for the stock RNS peer",
        )

        rate_result = run_checked(
            (str(prnsd), "path", "--config", str(config), "-r", "-j"),
            "Prnsd could not query stock RNS announce rates",
        )
        rate_row = require_row(
            load_rows(rate_result, "Prnsd did not return a valid announce-rate table"),
            RATE_HASH,
            "Prnsd announce-rate table omitted the stock RNS fixture",
        )
        require_evidence(
            rate_row.get("rate_violations") == 3
            and isinstance(rate_row.get("timestamps"), list)
            and len(rate_row["timestamps"]) == 2,
            "Prnsd did not decode the stock RNS announce-rate row",
        )

        transport_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_SERVER),
                    "identity-hash",
                    str(config / "storage/transport_identity"),
                ),
                "stock RNS could not read its transport identity",
            ),
            16,
            "stock RNS did not report a valid transport identity hash",
        )
        remote_path_result = run_checked(
            (
                str(prnsd),
                "path",
                "--config",
                str(config),
                "-t",
                "-j",
                "-R",
                transport_hash,
                "-i",
                str(management_identity),
            ),
            "Prnsd could not query the stock RNS remote path table",
        )
        require_row(
            load_rows(remote_path_result, "Prnsd did not return a valid remote path table"),
            peer_hash,
            "Prnsd remote path table omitted the stock RNS peer",
        )
        remote_rate_result = run_checked(
            (
                str(prnsd),
                "path",
                "--config",
                str(config),
                "-r",
                "-j",
                "-R",
                transport_hash,
                "-i",
                str(management_identity),
            ),
            "Prnsd could not query the stock RNS remote announce-rate table",
        )
        require_row(
            load_rows(
                remote_rate_result,
                "Prnsd did not return a valid remote announce-rate table",
            ),
            RATE_HASH,
            "Prnsd remote announce-rate table omitted the stock RNS fixture",
        )

        run_checked(
            (
                str(prnsd),
                "path",
                "--config",
                str(config),
                "-B",
                "--duration",
                "1",
                "--reason",
                "oracle",
                BLACKHOLE_HASH,
            ),
            "Prnsd could not add a stock RNS local blackhole",
        )
        blackhole_result = run_checked(
            (str(prnsd), "path", "--config", str(config), "-b"),
            "Prnsd could not list stock RNS local blackholes",
        )
        blackhole_pattern = rf"<{BLACKHOLE_HASH}> blackholed for .+ \(oracle\)"
        require_evidence(
            re.search(blackhole_pattern, blackhole_result) is not None,
            "Prnsd rendered unexpected local blackhole data",
        )
        published_result = run_checked(
            (
                str(prnsd),
                "path",
                "--config",
                str(config),
                "-p",
                transport_hash,
            ),
            "Prnsd could not query the stock RNS published blackhole list",
        )
        require_evidence(
            re.search(blackhole_pattern, published_result) is not None,
            "Prnsd rendered unexpected published blackhole data",
        )
        run_checked(
            (str(prnsd), "path", "--config", str(config), "-U", BLACKHOLE_HASH),
            "Prnsd could not remove a stock RNS local blackhole",
        )

        request_result = run_checked(
            (
                str(prnsd),
                "path",
                "--config",
                str(config),
                "-w",
                "10",
                peer_hash,
            ),
            "Prnsd could not request the stock RNS peer path",
        )
        require_output_marker(
            request_result,
            f"Path found, destination <{peer_hash}>",
            "Prnsd rendered unexpected path-request data",
        )
        run_checked(
            (str(prnsd), "path", "--config", str(config), "-x", via_hash),
            "Prnsd could not drop stock RNS paths through a transport",
        )
        drop_path_when_present(prnsd, config, peer_hash)
        run_checked(
            (str(prnsd), "path", "--config", str(config), "-D"),
            "Prnsd could not drop stock RNS announce queues",
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
