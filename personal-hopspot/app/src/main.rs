#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(all(target_arch = "xtensa", feature = "device-firehose"))]
mod bench_firehose;
#[cfg(any(
    all(target_arch = "xtensa", feature = "ble-bringup"),
    all(target_arch = "riscv32", feature = "ble-bringup-c6")
))]
mod ble;
#[cfg(not(target_os = "none"))]
mod desktop;
#[cfg(target_os = "none")]
mod engine_storage;
#[cfg(target_arch = "riscv32")]
mod esp32c6;
#[cfg(all(target_arch = "xtensa", not(feature = "device-firehose")))]
mod esp32s3;
#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    not(feature = "board-tbeam-supreme")
))]
mod heltec_v4;
#[cfg(not(target_os = "none"))]
mod host_serial;
#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    feature = "board-tbeam-supreme"
))]
mod t_beam_supreme;

#[cfg(not(target_os = "none"))]
fn main() {
    desktop::run();
}

#[cfg(target_arch = "riscv32")]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    esp32c6::run(spawner).await
}

#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    not(feature = "board-tbeam-supreme")
))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    heltec_v4::run(spawner).await
}

#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    feature = "board-tbeam-supreme"
))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    t_beam_supreme::run(spawner).await
}

#[cfg(all(target_arch = "xtensa", feature = "device-firehose"))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    bench_firehose::run(spawner).await
}
