import json
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
    require_evidence,
    require_hex_output,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_SERVER = ROOT / "validation/interop/peers/rns_rnstatus_server.py"
SUCCESS = "PASS: Prnsd status queried stock RNS local RPC and authenticated remote management"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with PortLease() as bus_port, PortLease() as control_port, InteropCase() as case:
        config = case.work / "config"
        management_identity = case.work / "management_identity"
        run_checked(
            (
                str(python),
                str(STOCK_SERVER),
                "prepare",
                str(config),
                str(bus_port.port),
                str(control_port.port),
                str(management_identity),
            ),
            "stock RNS did not prepare the rnstatus configuration",
        )
        bus_port.release()
        control_port.release()
        server = case.start_reference_rns(
            PeerSpec(
                "stock RNS rnstatus server",
                (str(python), str(STOCK_SERVER), "serve", str(config)),
                environment({}),
            )
        )
        case.wait_for(server, "RNSTATUS_SERVER_READY", 10)

        local_result = run_checked(
            (str(prnsd), "status", "--config", str(config), "--json"),
            "Prnsd could not query stock RNS shared-instance RPC",
        )
        try:
            report = json.loads(local_result)
        except json.JSONDecodeError as error:
            raise InteropFailure(
                FailureKind.EVIDENCE_MISSING,
                "Prnsd did not return a valid stock RNS status document",
            ) from error
        require_evidence(
            isinstance(report, dict)
            and isinstance(report.get("interfaces"), list)
            and bool(report.get("transport_id")),
            "Prnsd did not decode stock RNS local status",
        )

        transport_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_SERVER),
                    "identity-hash",
                    str(config / "storage/transport_identity"),
                ),
                "stock RNS could not read its transport identity",
            ),
            16,
            "stock RNS did not report a valid transport identity hash",
        )
        remote_result = run_checked(
            (
                str(prnsd),
                "status",
                "--config",
                str(config),
                "-R",
                transport_hash,
                "-i",
                str(management_identity),
                "-l",
                "-t",
            ),
            "Prnsd could not query stock RNS remote management",
        )
        require_output_marker(
            remote_result,
            f"Transport Instance <{transport_hash}> running",
            "Prnsd remote status did not identify the stock RNS transport",
        )
        require_output_marker(
            remote_result,
            "link table",
            "Prnsd remote status did not decode the stock RNS link count",
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
