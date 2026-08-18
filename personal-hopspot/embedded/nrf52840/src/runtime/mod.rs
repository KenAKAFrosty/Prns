#[cfg(feature = "board-t-echo")]
mod bluetooth_auto;
#[cfg(feature = "board-t-echo")]
mod bluetooth_gatt_server;
#[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
mod bootloader_entry;
mod entropy;
#[cfg(feature = "board-t-echo")]
mod firmware;
#[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
mod headless;
#[cfg(feature = "board-t-echo")]
mod interface_cards;
#[cfg(feature = "board-t-echo")]
pub(crate) mod node;

#[cfg(feature = "board-t-echo")]
pub use firmware::run;
#[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
pub use headless::run;
