//! The host RNode interface (RNS `RNodeInterface`): Reticulum packets pumped through a LoRa RNode
//! over a USB-serial KISS link. The host-agnostic [`core`] holds the radio configuration, the
//! command codec, the bring-up read-back model, and the descriptor; the per-host driver — which owns
//! the detect → configure → validate handshake and the data path — lives under [`impls`].
//!
//! This is the *host* side of an RNode (a PC driving a separate modem), distinct from the embedded
//! [`lora`](super::lora) interface, where the board itself is the radio.

pub mod core;
pub mod impls;
