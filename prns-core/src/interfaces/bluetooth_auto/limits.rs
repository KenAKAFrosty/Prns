//! Platform peer ceilings for the Bluetooth auto-interface.
//!
//! These are policy targets for settled Reticulum-over-BLE peers, not a claim that every controller
//! or OS release will always sustain that many live links.

pub const DESKTOP_MAX_PEERS: usize = 8;
pub const LINUX_MAX_PEERS: usize = DESKTOP_MAX_PEERS;
pub const MACOS_MAX_PEERS: usize = DESKTOP_MAX_PEERS;
pub const WINDOWS_MAX_PEERS: usize = DESKTOP_MAX_PEERS;

pub const ANDROID_MAX_PEERS: usize = 7;
pub const IOS_MAX_PEERS: usize = 7;

pub const ESP32_S3_MAX_PEERS: usize = 2;
// The C6 controller advertises a much higher theoretical connection ceiling, but the headless
// Hopspot build must keep USB-auto responsive while BLE is active on a single core/no-PSRAM board.
// Keep this paired with the C6-specific airtime/buffering profile in personal-hopspot/embedded/esp32/src/ble.rs.
pub const ESP32_C6_MAX_PEERS: usize = 20;
pub const T_ECHO_MAX_PEERS: usize = 5;
