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
STOCK_PEER = ROOT / "validation/interop/peers/rns_plain_group_peer.py"
STOCK_READY = "STOCK_PLAIN_GROUP_PEER_UP"
PRNS_READY = "PRNS_PLAIN_GROUP_PEER_UP"
STOCK_OK = "STOCK_PLAIN_GROUP_OK received_plain=1 received_group=1"
PRNS_OK = "PRNS_PLAIN_GROUP_OK received_plain=1 received_group=1"
SUCCESS = "PASS: stock RNS and Prns exchanged exact PLAIN and GROUP payloads in both directions"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS PLAIN/GROUP peer",
                (str(python), str(STOCK_PEER)),
                environment(
                    {
                        "PRNS_PLAIN_GROUP_PORT": port.port,
                        "PRNS_PLAIN_GROUP_CONFIG_DIR": case.work / "stock-rns",
                    }
                ),
            ),
            port,
        )
        case.wait_for(stock, STOCK_READY, 10)
        prns = case.start(
            PeerSpec(
                "Prns PLAIN/GROUP peer",
                (str(candidate), "plain-group"),
                environment({"PRNS_PLAIN_GROUP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for(prns, PRNS_READY, 10)
        case.wait_for_all([(stock, STOCK_OK), (prns, PRNS_OK)], 45)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
