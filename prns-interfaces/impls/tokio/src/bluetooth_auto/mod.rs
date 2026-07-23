mod host;
#[cfg(target_os = "linux")]
mod linux;
mod runtime;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use host::PreparedAutoBle;
pub use host::{AttachedBle, AutoBle, ConfiguredAutoBle};
#[cfg(target_os = "linux")]
pub use linux::{BluerBackend, BluerError};
pub use runtime::{BluetoothAuto, BluetoothAutoStatus, BluetoothPeer};
