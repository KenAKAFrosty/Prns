#![no_std]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

extern crate alloc;

#[cfg(all(
    target_arch = "xtensa",
    not(all(
        feature = "bluetooth-auto",
        feature = "esp-now",
        feature = "tcp",
        feature = "usb",
        feature = "wifi-auto"
    ))
))]
compile_error!(
    "ESP32-S3 firmware is built through a board package, which selects bluetooth-auto, esp-now, tcp, usb, and wifi-auto (plus lora on boards with an SX1262)"
);

#[cfg(all(
    target_arch = "riscv32",
    not(all(
        feature = "bluetooth-auto",
        feature = "esp-now",
        feature = "usb",
        not(feature = "lora"),
        not(feature = "tcp"),
        not(feature = "wifi-auto")
    ))
))]
compile_error!(
    "ESP32-C6 firmware is built through its board package, which selects bluetooth-auto, esp-now, and usb"
);

#[cfg(all(
    feature = "bluetooth-auto",
    any(target_arch = "riscv32", target_arch = "xtensa")
))]
pub mod bluetooth_auto;
#[cfg(all(
    target_arch = "riscv32",
    feature = "bluetooth-auto",
    feature = "esp-now",
    feature = "usb",
    not(feature = "lora"),
    not(feature = "tcp"),
    not(feature = "wifi-auto")
))]
pub mod c6;
#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
mod flash;
#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
mod identity;
#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
mod persistence;
#[cfg(all(
    target_arch = "xtensa",
    feature = "bluetooth-auto",
    feature = "esp-now",
    feature = "tcp",
    feature = "usb",
    feature = "wifi-auto"
))]
pub mod s3;
#[cfg(any(test, target_arch = "xtensa"))]
mod station_recovery;
#[cfg(any(target_arch = "riscv32", target_arch = "xtensa"))]
mod storage;
#[cfg(any(test, target_arch = "xtensa"))]
mod wifi_data_path_recovery;
