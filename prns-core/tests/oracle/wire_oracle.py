import json
import sys

import RNS


def decode(raw):
    packet = RNS.Packet(None, raw)
    if not packet.unpack():
        return {"error": "rejected"}
    header_length = 35 if packet.header_type == RNS.Packet.HEADER_2 else 19
    return {
        "ok": {
            "ifac_flag": (packet.flags >> 7) & 1,
            "header_type": packet.header_type,
            "context_flag": packet.context_flag,
            "propagation": packet.transport_type,
            "destination_type": packet.destination_type,
            "packet_type": packet.packet_type,
            "hops": packet.hops,
            "transport_id": packet.transport_id.hex() if packet.transport_id else None,
            "address": packet.destination_hash.hex(),
            "context": packet.context,
            "payload": packet.data.hex(),
        },
        "reencoded": (raw[:header_length] + packet.data).hex(),
    }


def main():
    corpus = json.load(sys.stdin)
    json.dump([decode(bytes.fromhex(raw)) for raw in corpus], sys.stdout)


if __name__ == "__main__":
    main()
