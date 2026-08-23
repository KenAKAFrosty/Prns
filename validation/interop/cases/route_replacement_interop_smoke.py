from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_example,
    case_main,
    environment,
    forbid_output_marker,
    reference_python,
    require_hex_output,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_TRANSPORT = ROOT / "validation/interop/peers/rns_multihop_transport.py"
STOCK_PEER = ROOT / "validation/interop/peers/rns_route_replacement_peer.py"
SUCCESS = "PASS: stock RNS selected a newer shorter path through Prns while the longer competing route remained connected"


def run() -> None:
    python = reference_python()
    daemon = cargo_example(MANIFEST, "mixed_multihop_daemon")
    with (
        PortLease() as long_port,
        PortLease() as short_port,
        PortLease() as relay_port,
        PortLease() as requester_port,
        InteropCase() as case,
    ):
        identity_path = case.work / "route.identity"
        destination_hash = require_hex_output(
            run_checked(
                (str(python), str(STOCK_PEER), "prepare", str(identity_path)),
                "stock RNS could not prepare the competing-route identity",
            ),
            16,
            "stock RNS returned an invalid competing-route destination hash",
        )
        long_announce = case.work / "announce-long"
        long_stop = case.work / "stop-long"
        long_verify = case.work / "verify-long"
        short_announce = case.work / "announce-short"
        long_endpoint = case.start_reference_rns(
            PeerSpec(
                "long-path stock RNS endpoint",
                (str(python), str(STOCK_PEER), "long"),
                environment(
                    {
                        "RNS_ROUTE_CONFIG_DIR": case.work / "long-config",
                        "RNS_ROUTE_IDENTITY_PATH": identity_path,
                        "RNS_ROUTE_PORT": long_port.port,
                        "RNS_ROUTE_ANNOUNCE_TRIGGER": long_announce,
                        "RNS_ROUTE_STOP_TRIGGER": long_stop,
                        "RNS_ROUTE_VERIFY_TRIGGER": long_verify,
                    }
                ),
            ),
            long_port,
        )
        case.wait_for(long_endpoint, "ROUTE_ENDPOINT_UP role=long", 10)
        short_endpoint = case.start_reference_rns(
            PeerSpec(
                "short-path stock RNS endpoint",
                (str(python), str(STOCK_PEER), "short"),
                environment(
                    {
                        "RNS_ROUTE_CONFIG_DIR": case.work / "short-config",
                        "RNS_ROUTE_IDENTITY_PATH": identity_path,
                        "RNS_ROUTE_PORT": short_port.port,
                        "RNS_ROUTE_ANNOUNCE_TRIGGER": short_announce,
                    }
                ),
            ),
            short_port,
        )
        case.wait_for(short_endpoint, "ROUTE_ENDPOINT_UP role=short", 10)
        relay = case.start_reference_rns(
            PeerSpec(
                "stock RNS long-path transport",
                (str(python), str(STOCK_TRANSPORT)),
                environment(
                    {
                        "RNS_MULTIHOP_LISTEN_PORT": relay_port.port,
                        "RNS_MULTIHOP_PEER_PORT": long_port.port,
                        "RNS_MULTIHOP_CONFIG_DIR": case.work / "relay-config",
                    }
                ),
            ),
            relay_port,
        )
        case.wait_for(relay, "MULTIHOP_TRANSPORT_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns competing-route transport",
                (str(daemon),),
                environment(
                    {
                        "PRNS_MULTIHOP_LISTEN_PORT": requester_port.port,
                        "PRNS_MULTIHOP_PEER": f"127.0.0.1:{relay_port.port}",
                        "PRNS_MULTIHOP_ALTERNATE_PEER": f"127.0.0.1:{short_port.port}",
                    }
                ),
            ),
            requester_port,
        )
        case.wait_for(prns, "MIXED_MULTIHOP_READY", 10)
        requester = case.start_reference_rns(
            PeerSpec(
                "stock RNS route requester",
                (str(python), str(STOCK_PEER), "requester"),
                environment(
                    {
                        "RNS_ROUTE_CONFIG_DIR": case.work / "requester-config",
                        "RNS_ROUTE_DESTINATION": destination_hash,
                        "RNS_ROUTE_PORT": requester_port.port,
                    }
                ),
            )
        )
        case.wait_for(requester, "ROUTE_REQUESTER_UP", 10)
        long_announce.write_text("announce", encoding="utf-8")
        case.wait_for(requester, "STOCK_ROUTE_INITIAL hops=3", 45)
        long_stop.write_text("stop", encoding="utf-8")
        case.wait_for(long_endpoint, "LONG_ROUTE_SILENT", 10)
        short_announce.write_text("announce", encoding="utf-8")
        case.wait_for_all(
            [
                (
                    requester,
                    "STOCK_ROUTE_REPLACEMENT_OK initial_hops=3 replacement_hops=2 proof=1",
                ),
                (short_endpoint, "SHORT_ROUTE_RECEIVED bytes=23"),
            ],
            45,
            required_peers=(long_endpoint, relay, prns),
        )
        forbid_output_marker(
            case.read_log(long_endpoint),
            "LONG_ROUTE_USED",
            "the replacement payload followed the incumbent long route",
        )
        long_verify.write_text("verify", encoding="utf-8")
        case.wait_for(long_endpoint, "LONG_ROUTE_CONNECTED count=1", 10)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
