#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]
#![forbid(unsafe_code)]

use embassy_executor::Spawner;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    personal_hopspot_esp32::s3::boards::heltec_e290::run(spawner).await
}
