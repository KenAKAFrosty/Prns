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
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_probe_client.py"
SUCCESS = "PASS: stock RNS received Prnsd's delivery proof from rnstransport.probe"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with PortLease() as port, InteropCase() as case:
        server_config = case.work / "server"
        client_config = case.work / "client"
        run_checked(
            (
                str(python),
                str(STOCK_CLIENT),
                "prepare",
                str(server_config),
                str(client_config),
                str(port.port),
            ),
            "stock RNS probe configuration was not prepared",
        )
        server = case.start(
            PeerSpec(
                "Prnsd probe responder",
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
                "probe",
                str(client_config),
                transport_hash,
            ),
            "stock RNS probe did not receive a valid delivery proof",
        )
        require_output_marker(
            result,
            "PROBE_RESPONDER_OK",
            "stock RNS probe did not report successful proof validation",
        )
        require_no_protocol_violations_output(result, "stock RNS probe client")


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
