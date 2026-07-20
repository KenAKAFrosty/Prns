#![no_std]
#![no_main]

use embassy_executor::Spawner;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    personal_hopspot_esp32::c6::run(spawner).await
}
