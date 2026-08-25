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
STOCK_PEER = ROOT / "validation/interop/peers/rns_large_request_server.py"
SUCCESS = "PASS: stock RNS and Prns completed Resource-backed Link.request responses both ways"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS large-request server",
                (str(python), str(STOCK_PEER)),
                environment({"PRNS_LARGE_REQUEST_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "LARGE_REQUEST_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns large-request client",
                (str(candidate), "large-request"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_LARGE_REQUEST_OK response=131072"),
                (prns, "PRNS_LARGE_REQUEST_OK response=131072 responded=131072"),
            ],
            50,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
