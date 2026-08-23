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
STOCK_PEER = ROOT / "validation/interop/peers/rns_blackhole_exchange.py"
SUCCESS = "PASS: stock RNS and Prnsd exchanged compatible blackhole lists in both directions"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with InteropCase() as case:
        with PortLease() as publisher_port:
            publisher_server = case.work / "prns-publisher"
            publisher_client = case.work / "stock-client"
            publisher_source = require_hex_output(
                run_checked(
                    (
                        str(python),
                        str(STOCK_PEER),
                        "prepare-prns-publisher",
                        str(publisher_server),
                        str(publisher_client),
                        str(publisher_port.port),
                    ),
                    "Prns blackhole publisher configuration was not prepared",
                ),
                16,
                "Prns blackhole publisher did not receive a valid source identity",
            )
            publisher = case.start(
                PeerSpec(
                    "Prnsd blackhole publisher",
                    (
                        str(prnsd),
                        "run",
                        "--log-format",
                        "json",
                        "--config",
                        str(publisher_server),
                    ),
                    environment({"RUST_LOG": "info"}),
                ),
                publisher_port,
            )
            case.wait_for(publisher, '"event":"daemon_ready', 15)
            publisher_result = run_checked(
                (
                    str(python),
                    str(STOCK_PEER),
                    "query",
                    str(publisher_client),
                    publisher_source,
                ),
                "stock RNS did not receive Prnsd's blackhole list",
            )
            require_output_marker(
                publisher_result,
                "BLACKHOLE_PUBLISHER_OK",
                "stock RNS did not validate Prnsd's blackhole list",
            )
            require_no_protocol_violations_output(
                publisher_result,
                "stock RNS blackhole query",
            )
            case.stop(publisher)

        with PortLease() as stock_port:
            stock_server = case.work / "stock-publisher"
            prns_client = case.work / "prns-client"
            stock_source = require_hex_output(
                run_checked(
                    (
                        str(python),
                        str(STOCK_PEER),
                        "prepare-stock-publisher",
                        str(stock_server),
                        str(prns_client),
                        str(stock_port.port),
                    ),
                    "stock RNS blackhole publisher configuration was not prepared",
                ),
                16,
                "stock RNS blackhole publisher did not create a valid source identity",
            )
            stock = case.start_reference_rns(
                PeerSpec(
                    "stock RNS blackhole publisher",
                    (str(python), str(STOCK_PEER), "serve", str(stock_server)),
                    environment({}),
                ),
                stock_port,
            )
            case.wait_for(stock, "BLACKHOLE_SERVER_READY", 15)
            updater = case.start(
                PeerSpec(
                    "Prnsd blackhole updater",
                    (
                        str(prnsd),
                        "run",
                        "--log-format",
                        "json",
                        "--config",
                        str(prns_client),
                    ),
                    environment({"RUST_LOG": "info"}),
                )
            )
            case.wait_for(updater, '"event":"daemon_ready', 15)
            source_file = prns_client / "storage/blackhole" / stock_source
            case.wait_for_path(updater, source_file, 50)
            updater_result = run_checked(
                (
                    str(python),
                    str(STOCK_PEER),
                    "verify-source-file",
                    str(source_file),
                    stock_source,
                ),
                "Prnsd's imported blackhole source file was not stock-compatible",
            )
            require_output_marker(
                updater_result,
                "BLACKHOLE_UPDATER_OK",
                "Prnsd did not persist a stock-compatible blackhole list",
            )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
