import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


def main() -> None:
    start_reference_reticulum(configdir=sys.argv[1], loglevel=None)
    print("STOCK_INSTANCE_UP", flush=True)
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
