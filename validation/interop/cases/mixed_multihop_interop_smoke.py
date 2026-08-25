from validation.interop.cases.mixed_transport_topology import (
    MixedTransportScenario,
    StartPolicy,
    no_preparation,
    run_mixed_transport,
)
from validation.interop.harness import case_main


LEFT_OK = "MULTIHOP_OK role=left hops=3 bytes=65536"
RIGHT_OK = "MULTIHOP_OK role=right hops=3 bytes=65536"
SUCCESS = "PASS: stock RNS endpoints exchanged exact Resources across stock and Prns transports at three path hops"


def run() -> None:
    run_mixed_transport(
        MixedTransportScenario(
            endpoint_mode="resources",
            left_success=LEFT_OK,
            right_success=RIGHT_OK,
            timeout_seconds=100,
            start_policy=StartPolicy.IMMEDIATE,
            prepare=no_preparation,
        )
    )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
