import struct
import sys

MAGIC_START0 = 0x0A324655
MAGIC_START1 = 0x9E5D5157
MAGIC_END = 0x0AB16F30
FLAG_FAMILY_ID = 0x00002000
PAYLOAD = 256
DATA_AREA = 476
BLOCK = 512


def to_uf2(data, base, family):
    remainder = len(data) % PAYLOAD
    if remainder:
        data = data + b"\x00" * (PAYLOAD - remainder)
    blocks = len(data) // PAYLOAD
    out = bytearray()
    for index in range(blocks):
        chunk = data[index * PAYLOAD:(index + 1) * PAYLOAD]
        header = struct.pack(
            "<IIIIIIII",
            MAGIC_START0,
            MAGIC_START1,
            FLAG_FAMILY_ID,
            base + index * PAYLOAD,
            PAYLOAD,
            index,
            blocks,
            family,
        )
        body = chunk + b"\x00" * (DATA_AREA - PAYLOAD)
        block = header + body + struct.pack("<I", MAGIC_END)
        if len(block) != BLOCK:
            raise ValueError(f"block {index} is {len(block)} bytes, expected {BLOCK}")
        out += block
    return bytes(out)


def main():
    if len(sys.argv) != 5:
        sys.exit("usage: bin2uf2.py <in.bin> <out.uf2> <base> <family>")
    source, destination, base, family = sys.argv[1:5]
    with open(source, "rb") as handle:
        data = handle.read()
    uf2 = to_uf2(data, int(base, 0), int(family, 0))
    with open(destination, "wb") as handle:
        handle.write(uf2)
    print(f"{destination}: {len(uf2)} bytes, {len(uf2) // BLOCK} blocks, base {base}, family {family}")


if __name__ == "__main__":
    main()
