#![no_std]
#![no_main]

use panic_halt as _;

use embassy_executor::Spawner;

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    personal_hopspot_nrf52840::run(spawner).await
}
