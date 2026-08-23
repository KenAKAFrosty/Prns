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
STOCK_PEER = ROOT / "validation/interop/peers/rns_announce_app_data_peer.py"
SUCCESS = "PASS: stock RNS and Prns each preserved exact opaque announce application bytes"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS announce peer",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "PRNS_ANNOUNCE_APP_DATA_PORT": port.port,
                        "PRNS_ANNOUNCE_APP_DATA_CONFIG_DIR": case.work / "stock-rns",
                    }
                ),
            ),
            port,
        )
        case.wait_for(stock, "ANNOUNCE_APP_DATA_PEER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns announce peer",
                (str(candidate), "announce-app-data"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_ANNOUNCE_APP_DATA_OK received=1"),
                (prns, "PRNS_ANNOUNCE_APP_DATA_OK received=1"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
