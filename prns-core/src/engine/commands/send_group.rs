use heapless::Vec as HeaplessVec;

use crate::routing::delivery::send_group::SendGroupWriteError;
use crate::wire::DestinationHash;

use super::{PrnsCommand, Settleable, Settlement, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN};

/// Conservative: RNS chunks every encrypted destination at one size.
pub const MAX_SEND_GROUP_PLAINTEXT_LEN: usize = MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN;

pub type SendGroupPayload = HeaplessVec<u8, MAX_SEND_GROUP_PLAINTEXT_LEN>;

/// RNS 1.4.2 `Packet(group_destination, data)`. Note a GROUP cannot prove, so the send is fire-and-forget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendGroup {
    pub destination: DestinationHash,
    pub payload: SendGroupPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGroupRejection {
    NoGroupKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGroupFailure {
    Rejected(SendGroupRejection),
    WriteFailed(SendGroupWriteError),
}

impl Settleable for SendGroup {
    type Success = ();
    type Failure = SendGroupFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendGroup(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SendGroupFailure>> {
        match settlement {
            Settlement::SendGroup(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}
