#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

#[cfg(target_arch = "xtensa")]
extern crate alloc;

#[cfg(all(target_arch = "xtensa", feature = "device-firehose"))]
mod bench_firehose;
#[cfg(all(target_arch = "xtensa", feature = "ble-bringup"))]
mod ble;
#[cfg(not(target_arch = "xtensa"))]
mod desktop;
#[cfg(target_arch = "xtensa")]
mod engine_storage;
#[cfg(all(target_arch = "xtensa", not(feature = "device-firehose")))]
mod esp32s3;
#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    not(feature = "board-tbeam-supreme")
))]
mod heltec_v4;
#[cfg(all(
    target_arch = "xtensa",
    not(feature = "device-firehose"),
    feature = "board-tbeam-supreme"
))]
mod t_beam_supreme;

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    desktop::run();
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
