use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::links::LinkId;

use super::{EngineCommand, Settleable, Settlement};

/// RNS 1.3.5 `Link.set_resource_strategy` as a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetResourceStrategy {
    pub link_id: LinkId,
    pub strategy: ResourceStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyFailure {
    Rejected(SetResourceStrategyError),
}

/// There is no `EngineCommand::SendResource`: resource payloads are borrowed slices far too large for the command lane, so sends enter through the host handle's `send_resource` streaming path and only their settlements ride the journal under these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceError {
    NoSuchLink,
    LinkNotActive,
    LinkBusy,
    TableFull,
    Build(BuildOutgoingResourceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceFailure {
    Rejected(SendResourceError),
    WriteFailed,
    /// The receiver sent `RESOURCE_RCL`; RNS 1.3.5 `Resource._rejected`.
    RejectedByPeer,
    Sequencing,
    Timeout,
}

impl Settleable for SetResourceStrategy {
    type Success = ();
    type Failure = SetResourceStrategyFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SetResourceStrategy(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SetResourceStrategyFailure>> {
        match settlement {
            Settlement::SetResourceStrategy(result) => Some(result),

            //We do this explicitly so that future new members must be re-considered, even if the common case is for them to end up here
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
            | Settlement::RpcQuery(_) => None,
        }
    }
}
