import os

from validation.interop.cases.local_transit_interop_smoke import (
    IfacConfiguration,
    run_transit,
)
from validation.interop.harness import case_main


SUCCESS = "PASS: IFAC rejected missing and incorrect credentials before bidirectional stock-RNS transit"


def run() -> None:
    run_transit(
        IfacConfiguration(
            network_name=os.environ.get("PRNS_IFAC_NETWORK_NAME", "prns-interop"),
            passphrase=os.environ.get("PRNS_IFAC_PASSPHRASE", "ifac-parity-secret"),
            size_bytes=int(os.environ.get("PRNS_IFAC_SIZE_BYTES", "16")),
        )
    )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
