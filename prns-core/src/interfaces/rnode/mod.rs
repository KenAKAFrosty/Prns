//! The host RNode interface (RNS `RNodeInterface`): a PC driving a separate LoRa modem over a
//! USB-serial KISS link, distinct from the embedded [`lora`](super::lora) interface where the
//! board itself is the radio. The host-agnostic [`core`] holds the radio configuration, codec,
//! read-back model, and descriptor; the per-host driver (detect, configure, validate, and the
//! data path) lives in `prns-interfaces-tokio`.

pub mod core;
