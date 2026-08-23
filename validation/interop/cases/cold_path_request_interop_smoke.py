from pathlib import Path

from validation.interop.cases.mixed_transport_topology import (
    STOCK_ENDPOINT,
    MixedTransportScenario,
    StartPolicy,
    run_mixed_transport,
)
from validation.interop.harness import (
    case_main,
    require_hex_output,
    run_checked,
)


LEFT_OK = "COLD_PATH_OK role=left hops=3 requests=1 proof=1"
RIGHT_OK = "COLD_PATH_OK role=right hops=3 requests=1 proof=1"
SUCCESS = "PASS: stock RNS endpoints discovered cold paths through stock and Prns transports and exchanged proven packets"


def prepare_identities(python: Path, work: Path) -> dict[str, dict[str, object]]:
    identity_paths = {
        "left": work / "left.identity",
        "right": work / "right.identity",
    }
    destination_hashes = {
        role: require_hex_output(
            run_checked(
                (str(python), str(STOCK_ENDPOINT), "prepare", role, str(path)),
                f"stock RNS could not prepare the {role} cold-path identity",
            ),
            16,
            f"stock RNS returned an invalid {role} cold-path destination hash",
        )
        for role, path in identity_paths.items()
    }
    return {
        "left": {
            "RNS_MULTIHOP_IDENTITY_PATH": identity_paths["left"],
            "RNS_MULTIHOP_REMOTE_DESTINATION": destination_hashes["right"],
        },
        "right": {
            "RNS_MULTIHOP_IDENTITY_PATH": identity_paths["right"],
            "RNS_MULTIHOP_REMOTE_DESTINATION": destination_hashes["left"],
        },
    }


def run() -> None:
    run_mixed_transport(
        MixedTransportScenario(
            endpoint_mode="cold-path",
            left_success=LEFT_OK,
            right_success=RIGHT_OK,
            timeout_seconds=90,
            start_policy=StartPolicy.SYNCHRONIZED,
            prepare=prepare_identities,
        )
    )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
