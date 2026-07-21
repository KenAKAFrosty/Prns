mod host;
#[cfg(target_os = "linux")]
mod linux;
mod runtime;

pub use host::{AttachedBle, AutoBle, ConfiguredAutoBle};
#[cfg(target_os = "linux")]
pub use linux::{BluerBackend, BluerError};
pub use runtime::{BluetoothAuto, BluetoothAutoStatus, BluetoothPeer};
