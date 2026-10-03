// The SX1262 boards only exist in a lora build: a board module must never have to invent a
// radio its hardware does not carry.
#[cfg(feature = "lora")]
pub mod heltec_e290;
#[cfg(feature = "lora")]
pub mod heltec_v4;
#[cfg(feature = "lora")]
pub mod heltec_v4_r8;
#[cfg(feature = "lora")]
pub mod t_beam_supreme;

#[cfg(feature = "lora")]
mod heltec_frontend;
