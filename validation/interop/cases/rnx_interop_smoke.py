from pathlib import Path

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    PeerSpec,
    PortLease,
    cargo_binary,
    case_main,
    environment,
    reference_python,
    reference_utility,
    require_hex_output,
    require_listening_destination,
    require_output_marker,
    run_checked,
    run_expect_status_with_streams,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_ORACLE = ROOT / "validation/interop/peers/rns_rnx_oracle.py"
SUCCESS = "PASS: Prnsd x exchanges authenticated execution requests and results with stock RNS rnx"


def portable_emit_command(python: Path, text: str) -> str:
    return f"'{python}' -c \"import sys; sys.stdout.write('{text}')\""


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    stock_rnx = reference_utility("rnx")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with (
        PortLease() as bus_port,
        PortLease() as control_port,
        PortLease() as network_port,
        InteropCase() as case,
    ):
        config = case.work / "config"
        client_config = case.work / "client-config"
        stock_identity = case.work / "stock.rid"
        client_identity = case.work / "client.rid"
        candidate_identity = case.work / "prns.rid"
        stock_destination = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "prepare",
                    str(config),
                    str(client_config),
                    str(bus_port.port),
                    str(control_port.port),
                    str(network_port.port),
                    str(stock_identity),
                    str(client_identity),
                ),
                "stock RNS did not prepare the rnx configuration",
            ),
            16,
            "stock RNS did not report a valid rnx destination",
        )
        bus_port.release()
        control_port.release()
        network_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS rnx server",
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "serve",
                    str(config),
                    str(stock_identity),
                ),
                environment({}),
            )
        )
        case.wait_for(server, f"RNX_SERVER_READY {stock_destination}", 10)

        candidate_result = run_expect_status_with_streams(
            (
                str(prnsd),
                "x",
                "--config",
                str(config),
                "-i",
                str(candidate_identity),
                "-m",
                "--stdin",
                "payload",
                stock_destination,
                "oracle-command",
            ),
            7,
            "Prnsd x did not mirror the stock RNS command status",
        )
        require_output_marker(
            candidate_result.standard_output,
            "identified:oracle-command:payload",
            "stock RNS did not decode the Prns execution request",
        )
        require_output_marker(
            candidate_result.standard_error,
            "stock-stderr",
            "Prnsd did not preserve stock RNS standard error",
        )

        candidate_destination = require_listening_destination(
            run_checked(
                (
                    str(prnsd),
                    "x",
                    "--config",
                    str(config),
                    "-i",
                    str(candidate_identity),
                    "-p",
                ),
                "Prnsd did not derive its rnx listener destination",
            ),
            "Prnsd did not report its rnx listener destination",
        )
        client_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "identity-hash",
                    str(client_identity),
                ),
                "stock RNS did not read the authorized rnx client identity",
            ),
            16,
            "stock RNS did not report a valid authorized rnx client hash",
        )
        listener = case.start(
            PeerSpec(
                "Prnsd authenticated rnx listener",
                (
                    str(prnsd),
                    "x",
                    "--config",
                    str(config),
                    "-i",
                    str(candidate_identity),
                    "-l",
                    "-a",
                    client_hash,
                ),
                environment({}),
            )
        )
        case.wait_for(listener, "x listening", 10)
        stock_result = run_checked(
            (
                str(stock_rnx),
                "--config",
                str(client_config),
                "-i",
                str(client_identity),
                "-w",
                "5",
                candidate_destination,
                portable_emit_command(python, "stock-to-prns"),
            ),
            "stock rnx could not execute through the Prns listener",
        )
        require_output_marker(
            stock_result,
            "stock-to-prns",
            "stock rnx did not decode the Prns execution response",
        )

        denied = case.start(
            PeerSpec(
                "unlisted stock RNS rnx client",
                (
                    str(stock_rnx),
                    "--config",
                    str(client_config),
                    "-i",
                    str(stock_identity),
                    "-w",
                    "2",
                    candidate_destination,
                    portable_emit_command(python, "denied"),
                ),
                environment({}),
            )
        )
        denied_status = case.wait_for_status(denied, 8)
        if denied_status == 0:
            raise InteropFailure(
                FailureKind.COMMAND_FAILED,
                "Prnsd accepted an unlisted stock RNS execution client",
            )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
