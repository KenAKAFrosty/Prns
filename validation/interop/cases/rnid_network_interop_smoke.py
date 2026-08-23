import re
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
    require_output_marker,
    run_checked,
    run_expect_status,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_SERVER = ROOT / "validation/interop/peers/rns_rnid_network_server.py"
READY_PATTERN = re.compile(
    r"^RNID_SERVER_READY ([0-9a-f]{32}) ([0-9a-f]{32}) ([0-9a-f]{32})$",
    re.MULTILINE,
)
SUCCESS = "PASS: Prnsd id resolved and announced identities through a stock RNS shared instance"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with PortLease() as bus_port, PortLease() as control_port, InteropCase() as case:
        config = case.work / "config"
        announce_identity = case.work / "announce.rid"
        run_checked(
            (
                str(python),
                str(STOCK_SERVER),
                "prepare",
                str(config),
                str(bus_port.port),
                str(control_port.port),
                str(announce_identity),
            ),
            "stock RNS did not prepare the identity-network configuration",
        )
        bus_port.release()
        control_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS identity server",
                (str(python), str(STOCK_SERVER), "serve", str(config)),
                environment({}),
            )
        )
        case.wait_for(server, "RNID_SERVER_READY ", 10)
        ready = READY_PATTERN.search(case.read_log(server))
        if ready is None:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "stock RNS did not report valid identity-network hashes",
            )
        identity_hash, destination_hash, announce_hash = ready.groups()

        identity_result = run_checked(
            (
                str(prnsd),
                "id",
                "--config",
                str(config),
                "-i",
                identity_hash,
                "-R",
                "-t",
                "5",
                "-p",
            ),
            "Prnsd could not resolve the stock RNS identity hash",
        )
        require_output_marker(
            identity_result,
            f"Identity Hash : <{identity_hash}>",
            "Prnsd returned the wrong identity for the stock RNS identity hash",
        )
        destination_result = run_checked(
            (
                str(prnsd),
                "id",
                "--config",
                str(config),
                "-i",
                destination_hash,
                "-R",
                "-t",
                "5",
                "-p",
            ),
            "Prnsd could not resolve the stock RNS destination hash",
        )
        require_output_marker(
            destination_result,
            f"Identity Hash : <{identity_hash}>",
            "Prnsd returned the wrong identity for the stock RNS destination hash",
        )

        no_cache_result = run_expect_status(
            (
                str(prnsd),
                "id",
                "--config",
                str(config),
                "-i",
                identity_hash,
                "-R",
                "-N",
                "-t",
                "1",
                "-p",
            ),
            2,
            "Prnsd --no-cache did not preserve the unresolved-identity exit status",
        )
        require_output_marker(
            no_cache_result,
            "could not get working identity",
            "Prnsd --no-cache did not bypass network identity resolution",
        )

        run_checked(
            (
                str(prnsd),
                "id",
                "--config",
                str(config),
                "-i",
                str(announce_identity),
                "-a",
                "oracle.identity",
            ),
            "Prnsd could not announce through the stock RNS shared instance",
        )
        case.wait_for(server, "RNID_ANNOUNCE_RECEIVED ", 5)
        announce_pattern = re.compile(
            rf"^RNID_ANNOUNCE_RECEIVED [0-9a-f]{{32}} {announce_hash}$",
            re.MULTILINE,
        )
        if announce_pattern.search(case.read_log(server)) is None:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "stock RNS did not receive the Prnsd identity announcement",
            )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
