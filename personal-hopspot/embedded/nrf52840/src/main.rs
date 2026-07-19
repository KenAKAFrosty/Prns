#![no_std]
#![no_main]

use panic_halt as _;

use embassy_executor::Spawner;

#[cfg(not(feature = "hopspot-t-echo"))]
mod development_profile;
#[cfg(feature = "hopspot-t-echo")]
mod hopspot_profile;
mod ssd1681;
mod storage;

#[cfg(feature = "hopspot-t-echo")]
#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    hopspot_profile::run(spawner).await
}

#[cfg(not(feature = "hopspot-t-echo"))]
#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    development_profile::run(spawner).await
}
