#!/usr/bin/env python3
import struct
import sys

UF2_MAGIC_START0 = 0x0A324655
UF2_MAGIC_START1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY = 0x00002000
PAYLOAD = 256


def convert(bin_path, base, family, out_path):
    with open(bin_path, "rb") as handle:
        data = handle.read()
    blocks = (len(data) + PAYLOAD - 1) // PAYLOAD
    out = bytearray()
    for index in range(blocks):
        chunk = data[index * PAYLOAD : (index + 1) * PAYLOAD]
        chunk = chunk + b"\x00" * (PAYLOAD - len(chunk))
        header = struct.pack(
            "<IIIIIIII",
            UF2_MAGIC_START0,
            UF2_MAGIC_START1,
            UF2_FLAG_FAMILY,
            base + index * PAYLOAD,
            PAYLOAD,
            index,
            blocks,
            family,
        )
        block = header + chunk + b"\x00" * (476 - PAYLOAD) + struct.pack("<I", UF2_MAGIC_END)
        out += block
    with open(out_path, "wb") as handle:
        handle.write(out)
    return blocks


def parse(value):
    return int(value, 16) if value.lower().startswith("0x") else int(value)


if __name__ == "__main__":
    args = dict(zip(sys.argv[2::2], sys.argv[3::2]))
    blocks = convert(
        sys.argv[1],
        parse(args["--base"]),
        parse(args["--family"]),
        args["--output"],
    )
    print(f"wrote {args['--output']} ({blocks} blocks @ {args['--base']})")
