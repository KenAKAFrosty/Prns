use heapless::Vec as HeaplessVec;

use crate::routing::links::channel::{channel_mdu, MessageType};
use crate::routing::links::data::LinkDataError;
use crate::routing::links::LinkId;

use super::{PacketReceiptDelivered, PrnsCommand, Settleable, Settlement};

pub const MAX_SEND_TO_CHANNEL_BODY_LEN: usize = channel_mdu(crate::wire::BROADCAST_MTU);

pub type SendToChannelBody = HeaplessVec<u8, MAX_SEND_TO_CHANNEL_BODY_LEN>;

/// RNS 1.4.2 `Channel.send()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendToChannel {
    pub link_id: LinkId,
    pub message_type: MessageType,
    pub body: SendToChannelBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToChannelRejection {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToChannelFailure {
    Rejected(SendToChannelRejection),
    WriteFailed(LinkDataError),
    WindowFull,
    Untrackable,
    Timeout,
}

impl Settleable for SendToChannel {
    type Success = PacketReceiptDelivered;
    type Failure = SendToChannelFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendToChannel(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<PacketReceiptDelivered, SendToChannelFailure>> {
        match settlement {
            Settlement::SendToChannel(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}
