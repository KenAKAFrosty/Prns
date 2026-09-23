import re
from pathlib import Path

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
    require_hex_output,
    require_no_protocol_violations_output,
    require_output_marker,
    run_checked,
)


ROOT = Path(__file__).resolve().parents[3]
INTEGRATION_MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_CLIENT = ROOT / "validation/interop/peers/rns_remote_management_client.py"
SUCCESS = "PASS: stock RNS queried a live Hopspot route through remote management"


def transport_hash(case: InteropCase, server) -> str:
    match = re.search(
        r"^HOPSPOT_RNS_PATH_READY transport=([0-9a-f]{32})$",
        case.read_log(server),
        re.MULTILINE,
    )
    if match is None:
        raise InteropFailure(
            FailureKind.EVIDENCE_MISSING,
            "Hopspot fixture did not report its transport identity hash",
        )
    return match.group(1)


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    hopspot = cargo_example(INTEGRATION_MANIFEST, "hopspot_remote_path")
    with PortLease() as port, InteropCase() as case:
        client_config = case.work / "client"
        management_identity = case.work / "management_identity"
        controller_public_key = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_CLIENT),
                    "prepare-hopspot",
                    str(client_config),
                    str(port.port),
                    str(management_identity),
                ),
                "stock RNS did not prepare the Hopspot path client",
            ),
            64,
            "stock RNS did not create a valid controller public key",
        )
        server = case.start(
            PeerSpec(
                "Hopspot remote-path server",
                (str(hopspot),),
                environment(
                    {
                        "PRNS_HOPSPOT_PORT": port.port,
                        "PRNS_CONTROLLER_PUBLIC_KEY": controller_public_key,
                    }
                ),
            ),
            port,
        )
        case.wait_for(server, "HOPSPOT_RNS_PATH_READY transport=", 10)
        result = run_checked(
            (
                str(python),
                str(STOCK_CLIENT),
                "query-hopspot",
                str(client_config),
                transport_hash(case, server),
                str(management_identity),
            ),
            "stock RNS could not query the Hopspot path table",
        )
        require_output_marker(
            result,
            "HOPSPOT_PATH_OK",
            "stock RNS did not observe its route in the Hopspot path table",
        )
        require_no_protocol_violations_output(result, "stock RNS Hopspot path client")


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
