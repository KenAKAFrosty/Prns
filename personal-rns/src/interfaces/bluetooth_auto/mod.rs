pub mod core;
pub mod impls;
pub mod seam;

#[cfg(feature = "embassy-bluetooth")]
pub use impls::embassy::{
    BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus, BluetoothMemberStatus,
};
