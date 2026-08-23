import os
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
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_shared_rpc_client.py"
SUCCESS = "PASS: stock RNS decoded Prns control-RPC replies"


def run_case(python: Path, daemon: Path, local_port: int, rpc_port: int) -> None:
    rpc_key = os.environ.get("PRNS_RPC_KEY", "5a" * 32)
    configured = environment(
        {
            "PRNS_LOCAL_PORT": local_port,
            "PRNS_RPC_PORT": rpc_port,
            "PRNS_RPC_KEY": rpc_key,
        }
    )
    with InteropCase() as case:
        server = case.start(
            PeerSpec(
                "Prns shared-instance RPC server",
                (str(daemon),),
                configured,
            )
        )
        case.wait_for(server, "READY shared-instance", 10)
        client = case.start_reference_rns(
            PeerSpec(
                "stock RNS RPC oracle",
                (str(python), str(STOCK_CLIENT)),
                configured,
            )
        )
        case.wait_for(client, "RPC_ORACLE_OK", 60)


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    daemon = cargo_example(MANIFEST, "local_shared_rpc_instance")
    configured_local = os.environ.get("PRNS_LOCAL_PORT")
    configured_rpc = os.environ.get("PRNS_RPC_PORT")
    if configured_local is not None and configured_rpc is not None:
        run_case(python, daemon, int(configured_local), int(configured_rpc))
        return
    with PortLease() as local, PortLease() as rpc:
        local.release()
        rpc.release()
        run_case(python, daemon, local.port, rpc.port)


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
