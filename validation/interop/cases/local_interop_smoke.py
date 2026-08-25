import re
from pathlib import Path

from validation.interop.cases.local_client_smoke import run as run_client_role
from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    cargo_example,
    case_main,
    environment,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_shared_instance_client.py"
ANNOUNCED = re.compile(r"^ANNOUNCED dest=([0-9a-f]{32})$", re.MULTILINE)
SUCCESS = "PASS: both Prns shared-instance roles carried proven stock-RNS application traffic"


def run_server_role() -> None:
    python = reference_python()
    daemon = cargo_example(MANIFEST, "local_shared_instance")
    with PortLease() as local, PortLease() as control, InteropCase() as case:
        local.release()
        control.release()
        server = case.start(
            PeerSpec(
                "Prns local shared-instance server",
                (str(daemon),),
                environment({"PRNS_LOCAL_PORT": local.port}),
            )
        )
        case.wait_for(server, "READY shared-instance", 10)
        client = case.start_reference_rns(
            PeerSpec(
                "stock RNS shared-instance client",
                (str(python), str(STOCK_CLIENT)),
                environment(
                    {
                        "PRNS_LOCAL_PORT": local.port,
                        "PRNS_RPC_PORT": control.port,
                    }
                ),
            )
        )
        case.wait_for(client, "ANNOUNCED dest=", 10)
        announced = ANNOUNCED.search(case.read_log(client))
        if announced is None:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "stock RNS client did not report a valid destination hash",
            )
        case.wait_for(
            server,
            f"HEARD dest={announced.group(1)} hops=0 kind=Some(LocalClient)",
            10,
        )
        case.wait_for_all(
            [
                (client, "STOCK_TO_PRNS_SHARED_OK bytes=27 proof=1"),
                (server, "PRNS_SHARED_SERVER_TRAFFIC_OK bytes=27"),
            ],
            30,
        )


def run() -> None:
    run_server_role()
    run_client_role()


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
