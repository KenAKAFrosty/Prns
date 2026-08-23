import os
from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    forbid_output_marker,
    reference_python,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_SERVER = ROOT / "validation/interop/peers/rns_shared_instance_server.py"
SUCCESS = "PASS: prnsd joined stock RNS as a client, started no interfaces, and decoded its control-RPC status"


def configuration(
    bus_port: int,
    control_port: int,
    rpc_key: str,
    interface_name: str,
    listener_port: int,
) -> str:
    return (
        "[reticulum]\n"
        "enable_transport = No\n"
        "share_instance = Yes\n"
        "shared_instance_type = tcp\n"
        f"shared_instance_port = {bus_port}\n"
        f"instance_control_port = {control_port}\n"
        f"rpc_key = {rpc_key}\n"
        "[interfaces]\n"
        f"[[{interface_name}]]\n"
        "type = TCPServerInterface\n"
        "interface_enabled = Yes\n"
        "listen_ip = 127.0.0.1\n"
        f"listen_port = {listener_port}\n"
    )


def run() -> None:
    python = reference_python()
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    rpc_key = os.environ.get("PRNS_RPC_KEY", "5a" * 32)
    with (
        PortLease() as bus,
        PortLease() as control,
        PortLease() as stock_listener,
        PortLease() as prns_listener,
        InteropCase() as case,
    ):
        stock_config = case.work / "stock"
        prns_config = case.work / "prns"
        stock_config.mkdir()
        prns_config.mkdir()
        (stock_config / "config").write_text(
            configuration(
                bus.port,
                control.port,
                rpc_key,
                "Stock Listener",
                stock_listener.port,
            ),
            encoding="utf-8",
        )
        (prns_config / "config").write_text(
            configuration(
                bus.port,
                control.port,
                rpc_key,
                "Listener",
                prns_listener.port,
            ),
            encoding="utf-8",
        )
        bus.release()
        control.release()
        stock_listener.release()
        prns_listener.release()
        stock = case.start(
            PeerSpec(
                "stock RNS shared-instance server",
                (str(python), str(STOCK_SERVER), str(stock_config)),
                environment({}),
            )
        )
        case.wait_for(stock, "STOCK_INSTANCE_UP", 20)
        daemon = case.start(
            PeerSpec(
                "Prnsd shared-instance client",
                (str(prnsd), "run", "--config", str(prns_config)),
                environment({"RUST_LOG": "info"}),
            )
        )
        case.wait_for_all(
            [
                (daemon, 'event="shared_instance_joined"'),
                (daemon, 'event="daemon_ready"'),
            ],
            20,
        )
        status = run_checked(
            (str(prnsd), "status", "--config", str(prns_config), "--json"),
            "prnsd did not decode the stock RNS control-RPC status",
        )
        require_output_marker(
            status,
            '"interfaces"',
            "prnsd status omitted the shared-instance interface report",
        )
        forbid_output_marker(
            case.read_log(daemon),
            'event="interface_started"',
            "prnsd started its configured interface while joined to stock RNS",
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
