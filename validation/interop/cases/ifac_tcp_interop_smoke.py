import os
from pathlib import Path

from validation.interop.cases.local_transit_interop_smoke import (
    IfacConfiguration,
    run_transit,
    transit_environment,
)
from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_example,
    case_main,
    forbid_output_marker,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_PEER = ROOT / "validation/interop/peers/rns_ifac_hostile.py"
SUCCESS = "PASS: IFAC rejected missing and incorrect credentials before bidirectional stock-RNS transit"


def candidate_server_rejection(ifac: IfacConfiguration) -> None:
    python = reference_python()
    peer = cargo_example(MANIFEST, "rns_interop_peer")
    with PortLease() as port, InteropCase() as case:
        port.release()
        server = case.start(
            PeerSpec(
                "Prns IFAC TCP server",
                (str(peer), "ifac-server"),
                transit_environment(
                    {"PRNS_IFAC_BIND": f"127.0.0.1:{port.port}"},
                    ifac,
                ),
            )
        )
        case.wait_for(server, "PRNS_IFAC_SERVER_UP", 10)

        matching_before = case.start_reference_rns(
            PeerSpec(
                "stock RNS matching IFAC client before rejection",
                (str(python), str(STOCK_PEER), "matching-before"),
                transit_environment({"PEER_TCP_PORT": port.port}, ifac),
            )
        )
        case.wait_for_all(
            [
                (matching_before, "MATCHING_IFAC_OK phase=before proof=1"),
                (server, "PRNS_IFAC_MATCHING_OK phase=before"),
            ],
            20,
        )
        case.wait_for_exit(matching_before, 10)

        for mode in ("missing", "wrong"):
            hostile = case.start_reference_rns(
                PeerSpec(
                    f"stock RNS {mode} IFAC client",
                    (str(python), str(STOCK_PEER), mode),
                    transit_environment({"PEER_TCP_PORT": port.port}, ifac),
                )
            )
            case.wait_for(hostile, f"HOSTILE_SENT {mode}", 10)
            case.require_running(
                hostile,
                1,
                f"{mode} IFAC client did not remain connected for protocol inspection",
            )
            case.wait_for_exit(hostile, 10)
            hostile_log = case.read_log(hostile)
            forbid_output_marker(
                hostile_log,
                "HOSTILE_PEER_ANNOUNCE",
                f"{mode} IFAC client received the candidate's authenticated announce",
            )
            forbid_output_marker(
                hostile_log,
                "HOSTILE_LINK_ACTIVE",
                f"{mode} IFAC client established a Link to the candidate",
            )
            forbid_output_marker(
                case.read_log(server),
                "FAILED AuthenticationBypass",
                f"candidate accepted the {mode} IFAC client's announce",
            )

        matching_after = case.start_reference_rns(
            PeerSpec(
                "stock RNS matching IFAC client after rejection",
                (str(python), str(STOCK_PEER), "matching-after"),
                transit_environment({"PEER_TCP_PORT": port.port}, ifac),
            )
        )
        case.wait_for_all(
            [
                (matching_after, "MATCHING_IFAC_OK phase=after proof=1"),
                (server, "PRNS_IFAC_MATCHING_OK phase=after"),
            ],
            20,
        )
        case.wait_for_exit(matching_after, 10)
        case.wait_for_exit(server, 10)


def run() -> None:
    ifac = IfacConfiguration(
        network_name=os.environ.get("PRNS_IFAC_NETWORK_NAME", "prns-interop"),
        passphrase=os.environ.get("PRNS_IFAC_PASSPHRASE", "ifac-parity-secret"),
        size_bytes=int(os.environ.get("PRNS_IFAC_SIZE_BYTES", "16")),
    )
    candidate_server_rejection(ifac)
    run_transit(ifac)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
