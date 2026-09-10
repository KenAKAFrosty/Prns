from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("check_compatibility.py")
SPEC = importlib.util.spec_from_file_location("check_compatibility", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not load {MODULE_PATH}")
compatibility = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compatibility)

CANONICAL_SOURCE = b"""
const fn ble_reticulum_uuid(last: u8) -> [u8; 16] {
    [
        0x37, 0x14, 0x5b, 0x00, 0x44, 0x2d, 0x4a, 0x94,
        0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, last,
    ]
}

pub const BLE_SERVICE_UUID_BYTES: [u8; 16] = ble_reticulum_uuid(0xe3);
"""


class BluetoothCompatibilityTests(unittest.TestCase):
    def test_extracts_canonical_service_uuid(self) -> None:
        self.assertEqual(
            compatibility.bluetooth_service_uuid(CANONICAL_SOURCE),
            "37145B00-442D-4A94-917F-8F42C5DA28E3",
        )

    def test_changed_service_suffix_changes_uuid(self) -> None:
        source = CANONICAL_SOURCE.replace(b"(0xe3)", b"(0xe4)")
        self.assertEqual(
            compatibility.bluetooth_service_uuid(source),
            "37145B00-442D-4A94-917F-8F42C5DA28E4",
        )

    def test_rejects_malformed_byte_count_and_literal(self) -> None:
        mutations = (
            CANONICAL_SOURCE.replace(b"0x28, last", b"last"),
            CANONICAL_SOURCE.replace(b"0x37,", b"0x137,"),
        )
        for source in mutations:
            with self.subTest(source=source):
                with self.assertRaisesRegex(ValueError, "canonical Bluetooth UUID"):
                    compatibility.bluetooth_service_uuid(source)

    def test_ignores_commented_and_string_decoys(self) -> None:
        active = CANONICAL_SOURCE.replace(b"(0xe3)", b"(0xe4)")
        block_decoy = b"/*\n" + CANONICAL_SOURCE + b"*/\n"
        line_decoy = b"\n".join(b"// " + line for line in CANONICAL_SOURCE.splitlines())
        string_decoy = b'const DECOY: &str = r#"' + CANONICAL_SOURCE + b'"#;\n'
        for prefix in (block_decoy, line_decoy, string_decoy):
            with self.subTest(prefix=prefix[:20]):
                self.assertEqual(
                    compatibility.bluetooth_service_uuid(prefix + active),
                    "37145B00-442D-4A94-917F-8F42C5DA28E4",
                )

    def test_manifest_requires_canonical_bluetooth_uuid(self) -> None:
        document = json.loads(compatibility.COMPATIBILITY_PATH.read_text(encoding="utf-8"))
        for bluetooth in (None, {"serviceUuid": "37145b00-442d-4a94-917f-8f42c5da28e3"}):
            mutated = json.loads(json.dumps(document))
            if bluetooth is None:
                mutated["prns"].pop("bluetoothAuto")
            else:
                mutated["prns"]["bluetoothAuto"] = bluetooth
            with self.subTest(bluetooth=bluetooth), tempfile.TemporaryDirectory() as temporary:
                path = pathlib.Path(temporary) / "compatibility.json"
                path.write_text(json.dumps(mutated), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "bluetoothAuto"):
                    compatibility.load_compatibility(path)


if __name__ == "__main__":
    unittest.main()
