use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::links::LinkId;

use super::{PrnsCommand, Settleable, Settlement};

/// RNS 1.4.2 `Link.set_resource_strategy` as a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetResourceStrategy {
    pub link_id: LinkId,
    pub strategy: ResourceStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyRejection {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyFailure {
    Rejected(SetResourceStrategyRejection),
}

/// There is no `PrnsCommand::SendResource`: resource payloads are borrowed slices far too large for the command lane, so sends enter through the host handle's `send_resource` streaming path and only their settlements ride the journal under these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceRejection {
    NoSuchLink,
    LinkNotActive,
    LinkBusy,
    TableFull,
    Build(BuildOutgoingResourceError),
    MetadataMisplaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceFailure {
    Rejected(SendResourceRejection),
    WriteFailed,
    /// The receiver sent `RESOURCE_RCL`; RNS 1.4.2 `Resource._rejected`.
    RejectedByPeer,
    Sequencing,
    Timeout,
    /// The staged continuation was never advertised, so no wire cancel rides out with this settlement.
    PredecessorFailed,
}

impl Settleable for SetResourceStrategy {
    type Success = ();
    type Failure = SetResourceStrategyFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SetResourceStrategy(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SetResourceStrategyFailure>> {
        match settlement {
            Settlement::SetResourceStrategy(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}
