#![no_std]
#![no_main]

use panic_halt as _;

use embassy_executor::Spawner;

mod hopspot;
mod ssd1681;
mod storage;

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    hopspot::run(spawner).await
}
