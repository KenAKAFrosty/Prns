//! The host-agnostic core of the reactor's plug-and-play USB-auto interface. It REUSES the
//! framing brain of [`crate::interfaces::impls::usb_auto::core`] wholesale — the `Prns`-magic
//! handshake (Hello / HelloAck) and the message-kind envelope over `rns_serial_framing` — so a
//! reactor host speaks the exact wire an unmigrated device already does, and the two still talk
//! across the cutover. Only the async loops under [`super::impls`] are fresh: a host that
//! discovers and multiplexes many CDC ports, a device that serves one link.

use crate::interfaces::rns_serial_framing::RnsSerialDecoder;

pub use crate::interfaces::impls::usb_auto::core::{
    decode_message, device_descriptor, host_descriptor, node_tag_for, Capabilities,
    MalformedMessage, Message, NodeTag, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES, READ_CHUNK_BYTES,
};

/// The device side's classification of one decoded message (`AnswerHandshake` / `Deliver` /
/// `Ignore`), reused from the legacy interface's core for the embassy device link.
#[cfg(any(test, feature = "embassy-contract"))]
pub use crate::interfaces::impls::usb_auto::core::{react_to, InboundReaction};

/// The streaming deframer one link feeds wire bytes into, yielding whole handshake/data frames.
pub type Decoder = RnsSerialDecoder<MAX_MESSAGE_BYTES>;

/// What a host does with one decoded inbound message from a port: answer a peer's probe, mark
/// the link confirmed on its acknowledgement, deliver a data frame, or ignore the rest.
pub enum HostInbound<'a> {
    AnswerHandshake,
    Confirmed(NodeTag),
    Data(&'a [u8]),
    Ignore,
}

/// Classify one decoded message from the host's point of view. A peer's `Hello` is answered
/// with our `HelloAck`; its `HelloAck` confirms the link (carrying its node tag); `Data` is the
/// payload; a malformed frame is dropped.
pub fn host_react(message: Result<Message<'_>, MalformedMessage>) -> HostInbound<'_> {
    match message {
        Ok(Message::Hello(_)) => HostInbound::AnswerHandshake,
        Ok(Message::HelloAck { tag, .. }) => HostInbound::Confirmed(tag),
        Ok(Message::Data(packet)) => HostInbound::Data(packet),
        Err(_) => HostInbound::Ignore,
    }
}
