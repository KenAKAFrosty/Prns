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
    /// The receiver sent `RESOURCE_RCL` — RNS 1.3.5 `Resource._rejected`,
    /// the other end refusing the offered transfer outright.
    RejectedByPeer,
    /// The receiver's hashmap-exhausted request named a position that closes
    /// no segment (the reference's "sequencing error"), which cancels the transfer.
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
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}
