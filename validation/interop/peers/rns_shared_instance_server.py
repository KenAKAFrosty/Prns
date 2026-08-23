import sys
import time

import RNS
from rns_protocol_evidence import start_reference_reticulum


def main() -> None:
    start_reference_reticulum(configdir=sys.argv[1], loglevel=None)
    identity = RNS.Identity()
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        "rnstransport",
        "probe",
    )
    destination.set_proof_strategy(RNS.Destination.PROVE_ALL)

    def receive(data, packet):
        if data != bytes([1]) * 24:
            raise RuntimeError(f"unexpected shared-instance probe payload {data!r}")
        print(f"STOCK_SHARED_CLIENT_TRAFFIC_OK bytes={len(data)}", flush=True)

    destination.set_packet_callback(receive)
    print(f"STOCK_INSTANCE_UP {destination.hash.hex()}", flush=True)
    while True:
        destination.announce()
        time.sleep(0.5)


if __name__ == "__main__":
    main()
