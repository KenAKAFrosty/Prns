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
STOCK_PEER = ROOT / "validation/interop/peers/rns_channel_server.py"
SUCCESS = "PASS: stock RNS and Prns exchanged ordered, proven Channel messages in both directions"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Channel server",
                (str(python), str(STOCK_PEER)),
                environment({"PRNS_CHANNEL_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "CHANNEL_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns Channel client",
                (str(candidate), "channel"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_CHANNEL_OK messages=2 ordered=1"),
                (stock, "STOCK_CHANNEL_ACKNOWLEDGED messages=2"),
                (prns, "PRNS_CHANNEL_OK messages=2 ordered=1 proven=2"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
