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
STOCK_PEER = ROOT / "validation/interop/peers/rns_ratchet_server.py"
SUCCESS = "PASS: stock RNS and Prns exchanged enforced packets across two ratchet generations"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS ratchet server",
                (str(python), str(STOCK_PEER)),
                environment({"PRNS_RATCHET_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "RATCHET_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns ratchet client",
                (str(candidate), "ratchet"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_RATCHET_OK received=2 distinct_ratchets=2 prns_proven=1"),
                (prns, "PRNS_RATCHET_OK sent=2 received=1 proven=2"),
            ],
            50,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
