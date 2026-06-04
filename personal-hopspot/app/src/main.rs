#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]

#[cfg(target_arch = "xtensa")]
extern crate alloc;

#[cfg(not(target_arch = "xtensa"))]
mod desktop;
#[cfg(target_arch = "xtensa")]
mod s3;

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    desktop::run();
}

#[cfg(target_arch = "xtensa")]
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    s3::run(spawner).await
}
