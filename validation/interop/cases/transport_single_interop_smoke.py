from validation.interop.cases.mixed_transport_topology import (
    MixedTransportScenario,
    StartPolicy,
    no_preparation,
    run_mixed_transport,
)
from validation.interop.harness import case_main


LEFT_OK = "TRANSPORT_SINGLE_OK role=left hops=3 sent=21 received=22 proof=1"
RIGHT_OK = "TRANSPORT_SINGLE_OK role=right hops=3 sent=22 received=21 proof=1"
SUCCESS = "PASS: stock RNS endpoints exchanged exact proven SINGLE packets through stock and Prns transports"


def run() -> None:
    run_mixed_transport(
        MixedTransportScenario(
            endpoint_mode="single",
            left_success=LEFT_OK,
            right_success=RIGHT_OK,
            timeout_seconds=90,
            start_policy=StartPolicy.SYNCHRONIZED,
            prepare=no_preparation,
        )
    )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
