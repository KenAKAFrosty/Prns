#![no_std]
#![no_main]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

extern crate alloc;

#[cfg(any(
    all(target_arch = "xtensa", feature = "ble-bringup"),
    all(target_arch = "riscv32", feature = "ble-bringup-c6")
))]
mod ble;
#[cfg(target_arch = "riscv32")]
mod c6;
#[cfg(target_arch = "xtensa")]
mod s3;
mod storage;

#[cfg(target_arch = "riscv32")]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    c6::run(spawner).await
}

#[cfg(all(target_arch = "xtensa", not(feature = "board-tbeam-supreme")))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    s3::boards::heltec_v4::run(spawner).await
}

#[cfg(all(target_arch = "xtensa", feature = "board-tbeam-supreme"))]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    s3::boards::t_beam_supreme::run(spawner).await
}
