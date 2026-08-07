mod bluetooth_auto;
mod bluetooth_gatt_server;
mod board;
mod display;
mod firmware;
mod identity;
mod input;
mod node;
pub(crate) mod persistence;

pub(super) use firmware::run;
