import sys
import time

import RNS


def main() -> None:
    RNS.Reticulum(configdir=sys.argv[1])
    print("STOCK_INSTANCE_UP", flush=True)
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
