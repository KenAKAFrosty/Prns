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
STOCK_PEER = ROOT / "validation/interop/peers/rns_link_closure_server.py"
SUCCESS = "PASS: stock RNS and Prns each closed an active Link and the remote peer observed it"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Link closure server",
                (str(python), str(STOCK_PEER)),
                environment({"PRNS_LINK_CLOSURE_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "LINK_CLOSURE_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns Link closure client",
                (str(candidate), "link-closure"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_OBSERVED_PRNS_CLOSE reason=initiator"),
                (stock, "STOCK_CLOSED_PRNS_LINK reason=destination"),
                (prns, "PRNS_OBSERVED_STOCK_CLOSE reason=peerClosed"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
