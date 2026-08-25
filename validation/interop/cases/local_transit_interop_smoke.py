from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_example,
    case_main,
    environment,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_PEER = ROOT / "validation/interop/peers/rns_transit_peer.py"
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_transit_client.py"
IFAC_VARIABLES = (
    "PRNS_IFAC_NETWORK_NAME",
    "PRNS_IFAC_PASSPHRASE",
    "PRNS_IFAC_SIZE_BYTES",
)
TRANSFER_TIMEOUT_SECONDS = 140
PEER_EXIT_TIMEOUT_SECONDS = 10
RESOURCE_BYTES = 1_000_000
RESOURCE_RECEIVED = f"RESOURCE_OK {RESOURCE_BYTES}"
RESOURCE_SENT = f"RESOURCE_SENT_OK {RESOURCE_BYTES}"
SUCCESS = "PASS: real RNS apps transferred multi-part resources through the shared instance both ways"


@dataclass(frozen=True)
class IfacConfiguration:
    network_name: str
    passphrase: str
    size_bytes: int


def transit_environment(
    values: Mapping[str, object],
    ifac: IfacConfiguration | None,
) -> dict[str, str]:
    configured = dict(values)
    if ifac is not None:
        configured.update(
            {
                "PRNS_IFAC_NETWORK_NAME": ifac.network_name,
                "PRNS_IFAC_PASSPHRASE": ifac.passphrase,
                "PRNS_IFAC_SIZE_BYTES": ifac.size_bytes,
            }
        )
    return environment(configured, without=IFAC_VARIABLES)


def run_transit(ifac: IfacConfiguration | None) -> None:
    python = reference_python()
    daemon = cargo_example(MANIFEST, "local_transit_daemon")
    with (
        PortLease() as peer_port,
        PortLease() as local_port,
        PortLease() as rpc_port,
        InteropCase() as case,
    ):
        peer = case.start_reference_rns(
            PeerSpec(
                "stock RNS transit peer",
                (str(python), str(STOCK_PEER)),
                transit_environment({"PEER_TCP_PORT": peer_port.port}, ifac),
            ),
            peer_port,
        )
        case.wait_for(peer, "PEER_DEST ", 20)
        local_port.release()
        rpc_port.release()
        bridge = case.start(
            PeerSpec(
                "Prns local transit bridge",
                (str(daemon),),
                transit_environment(
                    {
                        "PRNS_LOCAL_PORT": local_port.port,
                        "PRNS_RPC_PORT": rpc_port.port,
                        "PRNS_PEER_ADDR": f"127.0.0.1:{peer_port.port}",
                    },
                    ifac,
                ),
            )
        )
        case.wait_for(bridge, "READY bridge", 10)
        client = case.start_reference_rns(
            PeerSpec(
                "stock RNS local transit client",
                (str(python), str(STOCK_CLIENT)),
                transit_environment(
                    {
                        "PRNS_LOCAL_PORT": local_port.port,
                        "PRNS_RPC_PORT": rpc_port.port,
                    },
                    ifac,
                ),
            )
        )
        case.wait_for_all(
            [
                (peer, RESOURCE_RECEIVED),
                (peer, RESOURCE_SENT),
                (client, RESOURCE_RECEIVED),
                (client, RESOURCE_SENT),
                (bridge, "EGRESS_METRICS "),
            ],
            TRANSFER_TIMEOUT_SECONDS,
        )
        case.wait_for_exit(peer, PEER_EXIT_TIMEOUT_SECONDS)
        case.wait_for_exit(client, PEER_EXIT_TIMEOUT_SECONDS)


def run() -> None:
    run_transit(None)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
