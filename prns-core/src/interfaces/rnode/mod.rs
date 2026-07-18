//! The host RNode interface (RNS `RNodeInterface`): a PC driving a separate LoRa modem over a
//! USB-serial KISS link, distinct from the embedded [`lora`](super::lora) interface where the
//! board itself is the radio. The host-agnostic modules own radio configuration, codecs, bring-up,
//! and live protocol state; host adapters execute their typed actions over concrete transports.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
}

#[cfg(feature = "alloc")]
pub mod bring_up;
#[cfg(feature = "alloc")]
pub mod core;
#[cfg(feature = "alloc")]
pub mod live;
#[cfg(feature = "alloc")]
pub mod multi;
pub mod policy;
