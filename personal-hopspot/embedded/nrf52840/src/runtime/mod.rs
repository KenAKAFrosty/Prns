#[cfg(feature = "board-t-echo")]
mod bluetooth_auto;
#[cfg(feature = "board-t-echo")]
mod bluetooth_gatt_server;
mod entropy;
#[cfg(feature = "board-t-echo")]
mod firmware;
#[cfg(feature = "board-t114")]
mod headless;
#[cfg(feature = "board-t-echo")]
mod interface_cards;
#[cfg(feature = "board-t-echo")]
pub(crate) mod node;

#[cfg(feature = "board-t-echo")]
pub use firmware::run;
#[cfg(feature = "board-t114")]
pub use headless::run;
