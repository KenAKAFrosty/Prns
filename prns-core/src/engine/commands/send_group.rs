use heapless::Vec as HeaplessVec;

use crate::wire::DestinationHash;

use super::{EngineCommand, Settleable, Settlement, MAX_SEND_SINGLE_PLAINTEXT_LEN};

/// A GROUP destination shares the encrypted MDU; it carries no ephemeral key,
/// so this is conservative — the wire affords more, but RNS chunks every
/// encrypted destination at one size.
pub const MAX_SEND_GROUP_PLAINTEXT_LEN: usize = MAX_SEND_SINGLE_PLAINTEXT_LEN;

pub type SendGroupPayload = HeaplessVec<u8, MAX_SEND_GROUP_PLAINTEXT_LEN>;

/// One GROUP data packet, sealed with the destination's shared symmetric key and
/// broadcast to direct neighbors — RNS 1.3.1 `Packet(group_destination, data)`.
/// A GROUP cannot prove, so the send is fire-and-forget: it settles the moment it
/// is sealed and emitted, never on a delivery confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendGroup {
    pub destination: DestinationHash,
    pub payload: SendGroupPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGroupFailure {
    NoGroupKey,
    WriteFailed,
}

impl Settleable for SendGroup {
    type Success = ();
    type Failure = SendGroupFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendGroup(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SendGroupFailure>> {
        match settlement {
            Settlement::SendGroup(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}
