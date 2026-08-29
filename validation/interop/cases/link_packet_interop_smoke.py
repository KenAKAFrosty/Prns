from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    candidate_peer,
    case_main,
    environment,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
STOCK_PEER = ROOT / "validation/interop/peers/rns_link_packet_peer.py"
SUCCESS = "PASS: stock RNS and Prns each delivered and proved an exact direct Link packet"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Link packet peer",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "PRNS_LINK_PACKET_PORT": port.port,
                        "PRNS_LINK_PACKET_CONFIG_DIR": case.work / "stock-rns",
                    }
                ),
            ),
            port,
        )
        case.wait_for(stock, "LINK_PACKET_PEER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns Link packet peer",
                (str(candidate), "link-packet"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_LINK_PACKET_OK received=1 proof=1"),
                (prns, "PRNS_LINK_PACKET_OK received=1 proof=1"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
