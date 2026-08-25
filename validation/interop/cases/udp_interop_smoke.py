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
STOCK_PEER = ROOT / "validation/interop/peers/rns_udp_peer.py"
SUCCESS = "PASS: stock RNS and Prns UDP exchanged exact proven packets in both directions"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as stock_port, PortLease() as prns_port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS UDP peer",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "RNS_UDP_LOCAL_PORT": stock_port.port,
                        "RNS_UDP_PEER_PORT": prns_port.port,
                    }
                ),
            ),
            stock_port,
        )
        case.wait_for(stock, "UDP_PEER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns UDP peer",
                (str(candidate), "udp"),
                environment(
                    {
                        "PRNS_UDP_LOCAL": f"127.0.0.1:{prns_port.port}",
                        "PRNS_UDP_PEER": f"127.0.0.1:{stock_port.port}",
                    }
                ),
            ),
            prns_port,
        )
        case.wait_for_all(
            [
                (stock, "STOCK_UDP_OK received=1 proven=1"),
                (prns, "PRNS_UDP_OK received=1 proven=1"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
