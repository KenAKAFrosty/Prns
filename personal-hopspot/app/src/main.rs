#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

#[cfg(target_arch = "xtensa")]
extern crate alloc;

#[cfg(not(target_arch = "xtensa"))]
mod desktop;
#[cfg(target_arch = "xtensa")]
mod engine_storage;
#[cfg(all(target_arch = "xtensa", not(feature = "device-firehose")))]
mod s3;
#[cfg(all(target_arch = "xtensa", feature = "device-firehose"))]
mod bench_firehose;

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    desktop::run();
}

#[cfg(all(target_arch = "xtensa", not(feature = "device-firehose")))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    s3::run(spawner).await
}

#[cfg(all(target_arch = "xtensa", feature = "device-firehose"))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    bench_firehose::run(spawner).await
}
