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
STOCK_PEER = ROOT / "validation/interop/peers/rns_tunnel_recovery_peer.py"
SUCCESS = "PASS: stock RNS and Prns restored tunneled routes in both roles without fresh endpoint announces"


def stock_relay_direction(python: Path, candidate: Path) -> None:
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS tunnel relay",
                (str(python), str(STOCK_PEER), "relay"),
                environment(
                    {
                        "PRNS_TUNNEL_CONFIG_DIR": case.work / "stock-relay",
                        "PRNS_TUNNEL_PORT": port.port,
                    }
                ),
            ),
            port,
        )
        case.wait_for(stock, "STOCK_TUNNEL_RELAY_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns tunnel client",
                (str(candidate), "tunnel-recovery-client"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (
                    stock,
                    "STOCK_TUNNEL_RELAY_OK proof=2 announce_count=1 tunnel_reappeared=1",
                ),
                (prns, "PRNS_TUNNEL_RECOVERY_OK received=2 announce_count=1"),
            ],
            45,
        )


def prns_relay_direction(python: Path, candidate: Path) -> None:
    with PortLease() as port, InteropCase() as case:
        prns = case.start(
            PeerSpec(
                "Prns tunnel relay",
                (str(candidate), "tunnel-recovery-server"),
                environment({"PRNS_TCP_BIND": f"127.0.0.1:{port.port}"}),
            ),
            port,
        )
        case.wait_for(prns, "PRNS_TUNNEL_RELAY_UP", 10)
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS tunnel client",
                (str(python), str(STOCK_PEER), "client"),
                environment(
                    {
                        "PRNS_TUNNEL_CONFIG_DIR": case.work / "stock-client",
                        "PRNS_TUNNEL_PORT": port.port,
                    }
                ),
            )
        )
        case.wait_for_all(
            [
                (
                    prns,
                    "PRNS_TUNNEL_RELAY_OK proof=2 announce_count=1 route_repointed=1",
                ),
                (
                    stock,
                    "STOCK_TUNNEL_CLIENT_OK received=2 announce_count=1 reconnected=1",
                ),
            ],
            45,
        )


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    stock_relay_direction(python, candidate)
    prns_relay_direction(python, candidate)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
