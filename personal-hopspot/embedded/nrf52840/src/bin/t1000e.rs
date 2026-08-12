#![no_std]
#![no_main]

use panic_halt as _;

use embassy_executor::Spawner;

// The T1000-E build is scaffold-only: `runtime/firmware.rs` emits a
// `compile_error!` at the LoRaInterface construction because `LoRaInterface` is
// hard-wired to Sx126x and the T1000-E's LR1110 is pending a Radio trait
// generalization of `personal_rns::lora`. See T1000E_HOPSPOT_PORT.md.

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    personal_hopspot_nrf52840::run(spawner).await
}