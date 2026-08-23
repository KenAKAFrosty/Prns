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
STOCK_PEER = ROOT / "validation/interop/peers/rns_resource_rejection_peer.py"
SUCCESS = "PASS: stock RNS and Prns rejected Resource offers both ways without publishing bytes"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Resource rejection server",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "PRNS_REJECTION_ROLE": "reject-prns",
                        "PRNS_REJECTION_PORT": port.port,
                    }
                ),
            ),
            port,
        )
        case.wait_for(stock, "STOCK_REJECTION_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns Resource sender",
                (str(candidate), "resource-rejection-client"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_REJECTED_PRNS offers=1 published=0"),
                (prns, "PRNS_OBSERVED_STOCK_REJECTION published=0"),
            ],
            45,
        )
    with PortLease() as port, InteropCase() as case:
        prns = case.start(
            PeerSpec(
                "Prns Resource rejection server",
                (str(candidate), "resource-rejection-server"),
                environment({"PRNS_TCP_BIND": f"127.0.0.1:{port.port}"}),
            ),
            port,
        )
        case.wait_for(prns, "PRNS_REJECTION_SERVER_UP", 10)
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Resource sender",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "PRNS_REJECTION_ROLE": "send-to-prns",
                        "PRNS_REJECTION_PORT": port.port,
                    }
                ),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_OBSERVED_PRNS_REJECTION progress=0"),
                (prns, "PRNS_REJECTED_STOCK published=0"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
