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
STOCK_SERVER = ROOT / "validation/interop/peers/rns_tcp_server_peer.py"
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_tcp_client_peer.py"
SUCCESS = "PASS: stock RNS and Prns proved TCP client and server interoperability both ways"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS TCP server",
                (str(python), str(STOCK_SERVER)),
                environment({"PRNS_TCP_LISTEN_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns TCP client",
                (str(candidate), "tcp-client"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [(stock, "STOCK_TCP_SERVER_OK received=1"), (prns, "PRNS_TCP_CLIENT_OK proof=1")],
            45,
        )
    with PortLease() as port, InteropCase() as case:
        prns = case.start(
            PeerSpec(
                "Prns TCP server",
                (str(candidate), "tcp-server"),
                environment({"PRNS_TCP_BIND": f"127.0.0.1:{port.port}"}),
            ),
            port,
        )
        case.wait_for(prns, "PRNS_TCP_SERVER_UP", 10)
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS TCP client",
                (str(python), str(STOCK_CLIENT)),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [(stock, "PROVEN"), (prns, "PRNS_TCP_SERVER_OK received=1")],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
