use heapless::Vec as HeaplessVec;

use crate::routing::links::channel::{channel_mdu, MessageType};
use crate::routing::links::data::LinkDataError;
use crate::routing::links::LinkId;

use super::{Delivered, EngineCommand, Settleable, Settlement};

/// The most body bytes one channel message can carry at the broadcast MTU: the
/// channel MDU (the link MDU less the 6-byte envelope header).
pub const MAX_SEND_CHANNEL_BODY_LEN: usize = channel_mdu(crate::wire::BROADCAST_MTU);

pub type SendChannelBody = HeaplessVec<u8, MAX_SEND_CHANNEL_BODY_LEN>;

/// RNS 1.3.1 `Channel.send`: a sequenced, reliable message over a link's
/// channel. `message_type` is the envelope's opaque type tag (the engine never
/// interprets it); `body` is the message payload. Settles Delivered when the
/// peer's proof for this message arrives, or fails closed at emission
/// (no such link, window full) — the lost-in-flight timeout joins in slice 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendChannel {
    pub link_id: LinkId,
    pub message_type: MessageType,
    pub body: SendChannelBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendChannelError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendChannelFailure {
    Rejected(SendChannelError),
    WriteFailed(LinkDataError),
    /// The send window is full — the peer has not yet proved enough in-flight
    /// messages. The app retries once earlier sends settle.
    WindowFull,
    /// The channel table had no slot to track this link's channel.
    Untrackable,
    /// The retransmission budget ran out unproved — the link is being torn down.
    Timeout,
}

impl Settleable for SendChannel {
    type Success = Delivered;
    type Failure = SendChannelFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendChannel(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendChannelFailure>> {
        match settlement {
            Settlement::SendChannel(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}
