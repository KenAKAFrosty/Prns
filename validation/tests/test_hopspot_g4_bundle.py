"""Application packaging must reject incompatible and dynamically linked ELFs."""

import importlib.util
from pathlib import Path
import struct
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("hopspot_g4_build", ROOT / "tools/build/hopspot-g4.py")
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def static_mips_elf():
    data = bytearray(84)
    data[:7] = b"\x7fELF\x01\x01\x01"
    struct.pack_into("<HHIIIIIHHHHHH", data, 16, 2, 8, 1, 0, 52, 0, 0x70001007, 52, 32, 1, 0, 0, 0)
    struct.pack_into("<I", data, 52, 1)
    return data


class G4BundleTests(unittest.TestCase):
    def test_accepts_static_mips_o32(self):
        BUILD.verify_elf(static_mips_elf())

    def test_rejects_wrong_architecture_and_abi(self):
        for offset, fmt, value in [(4, "B", 2), (5, "B", 2), (16, "H", 3), (18, "H", 62), (36, "I", 0x70002000)]:
            with self.subTest(offset=offset, value=value):
                data = static_mips_elf()
                struct.pack_into("<" + fmt, data, offset, value)
                with self.assertRaises(ValueError):
                    BUILD.verify_elf(data)

    def test_rejects_dynamic_and_interpreter_segments(self):
        for segment in (2, 3):
            with self.subTest(segment=segment):
                data = static_mips_elf() + bytearray(32)
                struct.pack_into("<H", data, 44, 2)
                struct.pack_into("<I", data, 84, segment)
                with self.assertRaises(ValueError):
                    BUILD.verify_elf(data)

    def test_rejects_truncated_program_headers(self):
        data = static_mips_elf()
        for length in (0, 7, 51, 52, 83):
            with self.subTest(length=length), self.assertRaises(ValueError):
                BUILD.verify_elf(data[:length])

    def test_rejects_missing_load_segment(self):
        data = static_mips_elf()
        struct.pack_into("<I", data, 52, 0)
        with self.assertRaises(ValueError):
            BUILD.verify_elf(data)


if __name__ == "__main__":
    unittest.main()
