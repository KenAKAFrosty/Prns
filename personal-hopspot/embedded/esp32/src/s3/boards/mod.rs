// The SX1262 boards only exist in a lora build: a board module must never have to invent a
// radio its hardware does not carry.
#[cfg(feature = "lora")]
pub mod heltec_v4;
#[cfg(feature = "lora")]
pub mod heltec_v4_r8;
#[cfg(feature = "lora")]
pub mod t_beam_supreme;

// Both Heltec V4 front-end variants amplify the receive path before the SX1262. These typical
// gains are removed from its RSSI reports so channel access and diagnostics remain antenna-referred.
#[cfg(feature = "lora")]
pub(super) const HELTEC_GC1109_RX_GAIN_DB: u8 = 17;
#[cfg(feature = "lora")]
pub(super) const HELTEC_KCT8103L_RX_GAIN_DB: u8 = 23;
