use crate::engine::egress::EgressSerializeError;
use crate::units::HopCount;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

use super::{EngineCommand, Settleable, Settlement};

pub const PATH_REQUEST_ID_LEN: usize = TRUNCATED_HASH_BYTE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathRequestId([u8; PATH_REQUEST_ID_LEN]);

impl PathRequestId {
    pub const fn new(bytes: [u8; PATH_REQUEST_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; PATH_REQUEST_ID_LEN] {
        &self.0
    }
}

/// RNS 1.3.1 `Transport.request_path` — the structured form of the reference's
/// `await_path` poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPath {
    pub destination: DestinationHash,
    pub id: PathRequestId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathFound {
    pub hops: HopCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathFailure {
    WriteFailed(EgressSerializeError),
    Timeout,
    Culled,
}

impl Settleable for RequestPath {
    type Success = PathFound;
    type Failure = RequestPathFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::RequestPath(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<PathFound, RequestPathFailure>> {
        match settlement {
            Settlement::RequestPath(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{CommandId, CommandOutcome, IssuedCommand};

    #[test]
    fn a_request_path_owes_its_emission_for_any_destination() {
        let mut state = personal_node_announcer();
        let request = RequestPath {
            destination: DestinationHash::new([0x44; 16]),
            id: PathRequestId::new([0x55; 16]),
        };

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::RequestPath(request),
                },
                &[],
            ),
            CommandOutcome::OwesPathRequest {
                id: CommandId(7),
                request,
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn request_path_recovers_its_typed_settlement() {
        let verb = RequestPath {
            destination: DestinationHash::new([0x11; 16]),
            id: PathRequestId::new([0x22; 16]),
        };

        assert_eq!(verb.into_command(), EngineCommand::RequestPath(verb));
        assert_eq!(
            RequestPath::from_settlement(Settlement::RequestPath(Ok(PathFound {
                hops: HopCount(2)
            }))),
            Some(Ok(PathFound { hops: HopCount(2) })),
        );
        assert_eq!(
            RequestPath::from_settlement(Settlement::AnnounceNow(Ok(()))),
            None,
            "a path request never reads another verb's settlement",
        );
    }
}
