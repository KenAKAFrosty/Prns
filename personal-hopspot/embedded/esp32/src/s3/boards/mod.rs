pub mod heltec_v4;
pub mod heltec_v4_r8;
pub mod t_beam_supreme;

// Both Heltec V4 front-end variants amplify the receive path before the SX1262. These typical
// gains are removed from its RSSI reports so channel access and diagnostics remain antenna-referred.
pub(super) const HELTEC_GC1109_RX_GAIN_DB: u8 = 17;
pub(super) const HELTEC_KCT8103L_RX_GAIN_DB: u8 = 23;
