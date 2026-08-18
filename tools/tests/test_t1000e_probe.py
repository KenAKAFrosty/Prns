from __future__ import annotations

from importlib.machinery import SourceFileLoader
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
PROBE_PATH = ROOT / "tools" / "device" / "t1000e_probe.py"
LOADER = SourceFileLoader("t1000e_probe", str(PROBE_PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
assert SPEC is not None
probe = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = probe
LOADER.exec_module(probe)


class T1000EProbeTests(unittest.TestCase):
    def write_attribute(self, path: Path, name: str, value: str) -> None:
        path.mkdir(parents=True, exist_ok=True)
        (path / name).write_text(f"{value}\n", encoding="utf-8")

    def add_device(self, root: Path, name: str, vendor: str, product: str) -> Path:
        device = root / name
        self.write_attribute(device, "idVendor", vendor)
        self.write_attribute(device, "idProduct", product)
        return device

    def add_interface(self, root: Path, name: str, class_code: str) -> Path:
        interface = root / name
        self.write_attribute(interface, "bInterfaceClass", class_code)
        return interface

    def test_discovers_exact_bootloader_and_cdc_transport(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            sysfs_root = Path(temporary)
            self.add_device(sysfs_root, "1-1", "1234", "5678")
            device = self.add_device(sysfs_root, "3-1", "239a", "8029")
            self.write_attribute(device, "manufacturer", "Seeed Studio")
            self.write_attribute(device, "product", "T1000-E-BOOT")
            self.write_attribute(device, "serial", "EXACT-SERIAL")
            control = self.add_interface(sysfs_root, "3-1:1.0", "02")
            self.add_interface(sysfs_root, "3-1:1.1", "0a")
            (control / "tty" / "ttyACM7").mkdir(parents=True)

            self.assertEqual(
                probe.discover_bootloaders(sysfs_root),
                (
                    probe.T1000EBootloader(
                        sysfs_name="3-1",
                        manufacturer="Seeed Studio",
                        product="T1000-E-BOOT",
                        serial="EXACT-SERIAL",
                        interfaces=(
                            probe.UsbInterface(
                                name="3-1:1.0",
                                class_code="02",
                                kind=probe.UsbInterfaceKind.CDC_CONTROL,
                            ),
                            probe.UsbInterface(
                                name="3-1:1.1",
                                class_code="0a",
                                kind=probe.UsbInterfaceKind.CDC_DATA,
                            ),
                        ),
                        tty_names=("ttyACM7",),
                    ),
                ),
            )
            self.assertIs(
                probe.discover_bootloaders(sysfs_root)[0].transport,
                probe.BootloaderTransport.CDC_ONLY,
            )

    def test_mass_storage_interface_selects_uf2_transport(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            sysfs_root = Path(temporary)
            self.add_device(sysfs_root, "2-4", "239a", "8029")
            self.add_interface(sysfs_root, "2-4:1.0", "02")
            self.add_interface(sysfs_root, "2-4:1.1", "0a")
            self.add_interface(sysfs_root, "2-4:1.2", "08")

            bootloader = probe.discover_bootloaders(sysfs_root)[0]

            self.assertIs(
                bootloader.transport,
                probe.BootloaderTransport.UF2_MASS_STORAGE,
            )

    def test_missing_sysfs_root_is_an_inspection_error(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "missing"
            with self.assertRaises(FileNotFoundError):
                probe.discover_bootloaders(missing)


if __name__ == "__main__":
    unittest.main()
