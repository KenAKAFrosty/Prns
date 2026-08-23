from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    require_hex_output,
    require_no_protocol_violations_output,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_remote_management_client.py"
SUCCESS = "PASS: stock RNS rejected hostile management requests and recovered for valid queries"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with PortLease() as port, InteropCase() as case:
        server_config = case.work / "server"
        client_config = case.work / "client"
        management_identity = case.work / "management_identity"
        require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_CLIENT),
                    "prepare",
                    str(server_config),
                    str(client_config),
                    str(port.port),
                    str(management_identity),
                ),
                "stock RNS management configuration was not prepared",
            ),
            16,
            "stock RNS did not create a valid management identity",
        )
        server = case.start(
            PeerSpec(
                "Prnsd remote-management server",
                (str(prnsd), "run", "--log-format", "json", "--config", str(server_config)),
                environment({}),
            ),
            port,
        )
        case.wait_for_listener(server, "127.0.0.1", port.port, 10)
        transport_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_CLIENT),
                    "identity-hash",
                    str(server_config / "storage/transport_identity"),
                ),
                "stock RNS could not read the Prns transport identity",
            ),
            16,
            "stock RNS did not report a valid Prns transport identity hash",
        )
        result = run_checked(
            (
                str(python),
                str(STOCK_CLIENT),
                "query",
                str(client_config),
                transport_hash,
                str(management_identity),
            ),
            "stock RNS remote-management query did not complete",
        )
        require_output_marker(
            result,
            "REMOTE_MANAGEMENT_OK",
            "stock RNS did not report successful remote-management recovery",
        )
        require_no_protocol_violations_output(result, "stock RNS remote-management client")


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
