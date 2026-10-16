#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]
#![forbid(unsafe_code)]

use embassy_executor::Spawner;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    spawner.spawn(personal_hopspot_esp32::s3fn8::run(spawner).expect("firmware task fits"));
    core::future::pending().await
}
