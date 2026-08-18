from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum, IntEnum
from pathlib import Path


T1000E_BOOTLOADER_VENDOR_ID = "239a"
T1000E_BOOTLOADER_PRODUCT_ID = "8029"
USB_CDC_CONTROL_CLASS = "02"
USB_MASS_STORAGE_CLASS = "08"
USB_CDC_DATA_CLASS = "0a"


class UsbInterfaceKind(Enum):
    CDC_CONTROL = "CDC control"
    MASS_STORAGE = "mass storage"
    CDC_DATA = "CDC data"
    OTHER = "other"


class BootloaderTransport(Enum):
    UF2_MASS_STORAGE = "UF2 mass storage"
    CDC_ONLY = "CDC only"
    OTHER = "other"


class ProbeStatus(IntEnum):
    FOUND = 0
    NOT_FOUND = 1
    INSPECTION_FAILED = 2


@dataclass(frozen=True)
class UsbInterface:
    name: str
    class_code: str
    kind: UsbInterfaceKind


@dataclass(frozen=True)
class T1000EBootloader:
    sysfs_name: str
    manufacturer: str | None
    product: str | None
    serial: str | None
    interfaces: tuple[UsbInterface, ...]
    tty_names: tuple[str, ...]

    @property
    def transport(self) -> BootloaderTransport:
        kinds = {interface.kind for interface in self.interfaces}
        if UsbInterfaceKind.MASS_STORAGE in kinds:
            return BootloaderTransport.UF2_MASS_STORAGE
        if kinds and kinds <= {
            UsbInterfaceKind.CDC_CONTROL,
            UsbInterfaceKind.CDC_DATA,
        }:
            return BootloaderTransport.CDC_ONLY
        return BootloaderTransport.OTHER


def read_attribute(path: Path, name: str) -> str | None:
    try:
        return (path / name).read_text(encoding="utf-8").strip()
    except OSError:
        return None


def interface_kind(class_code: str) -> UsbInterfaceKind:
    if class_code == USB_CDC_CONTROL_CLASS:
        return UsbInterfaceKind.CDC_CONTROL
    if class_code == USB_MASS_STORAGE_CLASS:
        return UsbInterfaceKind.MASS_STORAGE
    if class_code == USB_CDC_DATA_CLASS:
        return UsbInterfaceKind.CDC_DATA
    return UsbInterfaceKind.OTHER


def read_interfaces(sysfs_root: Path, device_name: str) -> tuple[UsbInterface, ...]:
    interfaces = []
    for path in sorted(sysfs_root.glob(f"{device_name}:*")):
        class_code = read_attribute(path, "bInterfaceClass")
        if class_code is None:
            continue
        interfaces.append(
            UsbInterface(
                name=path.name,
                class_code=class_code,
                kind=interface_kind(class_code),
            )
        )
    return tuple(interfaces)


def read_tty_names(
    sysfs_root: Path,
    interfaces: tuple[UsbInterface, ...],
) -> tuple[str, ...]:
    tty_names = set()
    for interface in interfaces:
        tty_directory = sysfs_root / interface.name / "tty"
        try:
            tty_names.update(path.name for path in tty_directory.iterdir())
        except OSError:
            continue
    return tuple(sorted(tty_names))


def discover_bootloaders(sysfs_root: Path) -> tuple[T1000EBootloader, ...]:
    bootloaders = []
    device_paths = sorted(sysfs_root.iterdir())
    for path in device_paths:
        if read_attribute(path, "idVendor") != T1000E_BOOTLOADER_VENDOR_ID:
            continue
        if read_attribute(path, "idProduct") != T1000E_BOOTLOADER_PRODUCT_ID:
            continue
        interfaces = read_interfaces(sysfs_root, path.name)
        bootloaders.append(
            T1000EBootloader(
                sysfs_name=path.name,
                manufacturer=read_attribute(path, "manufacturer"),
                product=read_attribute(path, "product"),
                serial=read_attribute(path, "serial"),
                interfaces=interfaces,
                tty_names=read_tty_names(sysfs_root, interfaces),
            )
        )
    return tuple(bootloaders)


def display_value(value: str | None) -> str:
    return value if value else "unavailable"


def render_bootloader(bootloader: T1000EBootloader, dev_root: Path) -> str:
    interface_summary = ", ".join(
        f"{interface.kind.value} ({interface.class_code})"
        for interface in bootloader.interfaces
    )
    if not interface_summary:
        interface_summary = "none"
    tty_summary = ", ".join(
        f"{dev_root / name} ({'available' if (dev_root / name).exists() else 'not exposed'})"
        for name in bootloader.tty_names
    )
    if not tty_summary:
        tty_summary = "none"
    uf2_interface = (
        "present"
        if bootloader.transport is BootloaderTransport.UF2_MASS_STORAGE
        else "absent"
    )
    return "\n".join(
        (
            f"T1000-E bootloader: {bootloader.sysfs_name}",
            f"USB: {T1000E_BOOTLOADER_VENDOR_ID}:{T1000E_BOOTLOADER_PRODUCT_ID}",
            f"Manufacturer: {display_value(bootloader.manufacturer)}",
            f"Product: {display_value(bootloader.product)}",
            f"Serial: {display_value(bootloader.serial)}",
            f"Interfaces: {interface_summary}",
            f"Bootloader transport: {bootloader.transport.value}",
            f"UF2 mass-storage interface: {uf2_interface}",
            f"Serial devices: {tty_summary}",
        )
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--sysfs-root",
        type=Path,
        default=Path("/sys/bus/usb/devices"),
    )
    parser.add_argument("--dev-root", type=Path, default=Path("/dev"))
    return parser.parse_args()


def main() -> ProbeStatus:
    arguments = parse_args()
    try:
        bootloaders = discover_bootloaders(arguments.sysfs_root)
    except OSError as error:
        print(f"Cannot inspect USB devices at {arguments.sysfs_root}: {error}")
        return ProbeStatus.INSPECTION_FAILED
    if not bootloaders:
        print("No T1000-E bootloader found.")
        return ProbeStatus.NOT_FOUND
    print(f"T1000-E bootloaders found: {len(bootloaders)}")
    for index, bootloader in enumerate(bootloaders):
        if index:
            print()
        print(render_bootloader(bootloader, arguments.dev_root))
    return ProbeStatus.FOUND


if __name__ == "__main__":
    raise SystemExit(main())
