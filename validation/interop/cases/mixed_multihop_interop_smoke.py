from pathlib import Path

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
STOCK_TRANSPORT = ROOT / "validation/interop/peers/rns_multihop_transport.py"
STOCK_ENDPOINT = ROOT / "validation/interop/peers/rns_multihop_endpoint.py"
LEFT_OK = "MULTIHOP_OK role=left hops=3 bytes=65536"
RIGHT_OK = "MULTIHOP_OK role=right hops=3 bytes=65536"
SUCCESS = "PASS: stock RNS endpoints exchanged exact Resources across stock and Prns transports at three path hops"


def run() -> None:
    python = reference_python()
    daemon = cargo_example(MANIFEST, "mixed_multihop_daemon")
    with (
        PortLease() as left_port,
        PortLease() as prns_port,
        PortLease() as right_port,
        InteropCase() as case,
    ):
        right = case.start(
            PeerSpec(
                "right stock RNS endpoint",
                (str(python), str(STOCK_ENDPOINT)),
                environment(
                    {
                        "RNS_MULTIHOP_ROLE": "right",
                        "RNS_MULTIHOP_ENDPOINT_PORT": right_port.port,
                    }
                ),
            ),
            right_port,
        )
        case.wait_for(right, "MULTIHOP_ENDPOINT_UP role=right", 10)
        prns = case.start(
            PeerSpec(
                "Prns multihop transport",
                (str(daemon),),
                environment(
                    {
                        "PRNS_MULTIHOP_LISTEN_PORT": prns_port.port,
                        "PRNS_MULTIHOP_PEER": f"127.0.0.1:{right_port.port}",
                    }
                ),
            ),
            prns_port,
        )
        case.wait_for(prns, "MIXED_MULTIHOP_READY", 10)
        transport = case.start(
            PeerSpec(
                "stock RNS multihop transport",
                (str(python), str(STOCK_TRANSPORT)),
                environment(
                    {
                        "RNS_MULTIHOP_LISTEN_PORT": left_port.port,
                        "RNS_MULTIHOP_PEER_PORT": prns_port.port,
                    }
                ),
            ),
            left_port,
        )
        case.wait_for(transport, "MULTIHOP_TRANSPORT_UP", 10)
        left = case.start(
            PeerSpec(
                "left stock RNS endpoint",
                (str(python), str(STOCK_ENDPOINT)),
                environment(
                    {
                        "RNS_MULTIHOP_ROLE": "left",
                        "RNS_MULTIHOP_ENDPOINT_PORT": left_port.port,
                    }
                ),
            )
        )
        case.wait_for_all(
            [(left, LEFT_OK), (right, RIGHT_OK)],
            100,
            required_peers=(prns, transport),
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
