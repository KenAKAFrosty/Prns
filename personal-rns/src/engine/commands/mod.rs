//! App-issued commands, ingested by the engine as plain data.
//! Commands cross thread, task, and FFI boundaries as owned values,
//! so any host can queue them and the engine cycle drains them deterministically.
//!
//! RNS 1.3.1 has no scheduled announces at all: `Destination.announce()` is
//! app-called, and periodic announcing lives in app land (LXMF runs its own
//! timers). So [`AnnounceNow`] is the reference primitive, and this engine's
//! re-announce schedule is the extension built ahead of it.

use crate::engine::egress::EgressSerializeError;
use crate::engine::self_announce::SelfAnnounceAppData;
use crate::engine::send_single::WriteSendSingleError;
use crate::engine::WriteSelfAnnounceError;
use crate::interfaces::InterfaceId;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};
use heapless::Vec as HeaplessVec;

/// Ephemeral correlation for one issued command: minted by the caller (a
/// queued command has no return channel at submit time), echoed opaquely
/// through every outcome, never inspected by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCommand {
    pub id: CommandId,
    pub command: EngineCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
    SendSingle(SendSingle),
    RequestPath(RequestPath),
}

/// `Destination.announce(app_data=…, attached_interface=…)` as data
/// (RNS 1.3.1 Destination.py).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceNow {
    pub destination: DestinationHash,
    pub target: AnnounceTarget,
    pub app_data: AnnounceAppData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceTarget {
    AllInterfaces,
    Interface(InterfaceId),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnounceAppData {
    Scheduled,
    Data(SelfAnnounceAppData),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    OwesAnnounce {
        id: CommandId,
        announce: AnnounceNow,
    },
    AnnounceRejected {
        id: CommandId,
        error: AnnounceNowError,
    },
    OwesSendSingle {
        id: CommandId,
        send: SendSingle,
    },
    SendSingleRejected {
        id: CommandId,
        error: SendSingleError,
    },
    OwesPathRequest {
        id: CommandId,
        request: RequestPath,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleError {
    NoRouteToDestination,
    NotDirectlyReachable,
}

/// RNS 1.3.1 `Packet.ENCRYPTED_MDU` (383): the most plaintext one encrypted
/// Single data packet can carry — MDU minus the token overhead (32B ephemeral
/// key, 16B IV, 32B MAC), floored to a whole AES block, minus one pad byte.
pub const MAX_SEND_SINGLE_PLAINTEXT_LEN: usize = 383;

pub type SendSinglePayload = HeaplessVec<u8, MAX_SEND_SINGLE_PLAINTEXT_LEN>;

/// One Single data packet to a peer whose announce we hold, proof expected
/// back — RNS 1.3.1 `Packet(destination, data).send()` with its
/// `PacketReceipt`. Settles when the proof arrives, the timeout passes, or
/// the receipt is culled — never in its own cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendSingle {
    pub destination: DestinationHash,
    pub payload: SendSinglePayload,
}

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

/// RNS 1.3.1 `Transport.request_path`: ask the network for a path to
/// `destination`. A broadcast plain packet, answered by any reachable peer that
/// holds the path (re-)announcing it. Fire-and-forget — it settles the moment
/// it leaves, since the answer arrives later as an ordinary announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPath {
    pub destination: DestinationHash,
    pub id: PathRequestId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowError {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    UnknownInterface,
}

/// The terminal result of one issued command, paired verb-for-verb with
/// [`EngineCommand`] so every verb's success and failure stay typed across the
/// event lane — a data boundary erases type-level ties, so the tie is explicit
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    AnnounceNow(Result<(), AnnounceNowFailure>),
    SendSingle(Result<Delivered, SendSingleFailure>),
    RequestPath(Result<(), RequestPathFailure>),
}

/// RNS 1.3.1 `PacketReceipt.DELIVERED`, with the round trip it measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub rtt_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleFailure {
    Rejected(SendSingleError),
    WriteFailed(WriteSendSingleError),
    Culled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowFailure {
    Rejected(AnnounceNowError),
    WriteFailed(WriteSelfAnnounceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathFailure {
    WriteFailed(EgressSerializeError),
}

pub trait Settleable {
    type Success;
    type Failure;

    fn into_command(self) -> EngineCommand;
    fn from_settlement(settlement: Settlement) -> Option<Result<Self::Success, Self::Failure>>;
}

impl Settleable for AnnounceNow {
    type Success = ();
    type Failure = AnnounceNowFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::AnnounceNow(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), AnnounceNowFailure>> {
        match settlement {
            Settlement::AnnounceNow(result) => Some(result),
            Settlement::SendSingle(_) | Settlement::RequestPath(_) => None,
        }
    }
}

impl Settleable for SendSingle {
    type Success = Delivered;
    type Failure = SendSingleFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendSingle(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendSingleFailure>> {
        match settlement {
            Settlement::SendSingle(result) => Some(result),
            Settlement::AnnounceNow(_) | Settlement::RequestPath(_) => None,
        }
    }
}

impl Settleable for RequestPath {
    type Success = ();
    type Failure = RequestPathFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::RequestPath(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), RequestPathFailure>> {
        match settlement {
            Settlement::RequestPath(result) => Some(result),
            Settlement::AnnounceNow(_) | Settlement::SendSingle(_) => None,
        }
    }
}

use crate::engine::self_announce::MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN;
use crate::engine::EngineState;
use crate::interfaces::InterfaceDescriptor;
use crate::routing::storage::EngineStorage;
use crate::wire::DestinationType;

impl<S: EngineStorage> EngineState<S> {
    #[must_use]
    pub fn ingest_command(
        &mut self,
        issued: IssuedCommand,
        interfaces: &[InterfaceDescriptor],
    ) -> CommandOutcome {
        self.ingested_command_count = self.ingested_command_count.saturating_add(1);
        let IssuedCommand { id, command } = issued;
        match command {
            EngineCommand::AnnounceNow(announce_now) => {
                self.ingest_announce_now(id, announce_now, interfaces)
            }
            EngineCommand::SendSingle(send) => self.ingest_send_single(id, send),
            EngineCommand::RequestPath(request) => CommandOutcome::OwesPathRequest { id, request },
        }
    }

    fn ingest_announce_now(
        &self,
        id: CommandId,
        announce_now: AnnounceNow,
        interfaces: &[InterfaceDescriptor],
    ) -> CommandOutcome {
        if self
            .upstream_app_destinations
            .lookup(&announce_now.destination, DestinationType::Single)
            .is_none()
        {
            return CommandOutcome::AnnounceRejected {
                id,
                error: if self
                    .upstream_app_destinations
                    .lookup(&announce_now.destination, DestinationType::Plain)
                    .is_some()
                {
                    AnnounceNowError::NotASingleDestination
                } else {
                    AnnounceNowError::UnknownDestination
                },
            };
        }
        if let AnnounceTarget::Interface(interface) = announce_now.target {
            if !interfaces
                .iter()
                .any(|descriptor| descriptor.id == interface)
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    error: AnnounceNowError::UnknownInterface,
                };
            }
        }
        if let AnnounceAppData::Data(data) = &announce_now.app_data {
            if self.self_ratchets.is_tracked(&announce_now.destination)
                && data.len() > MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    error: AnnounceNowError::AppDataTooLong,
                };
            }
        }
        CommandOutcome::OwesAnnounce {
            id,
            announce: announce_now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::RatchetPolicy;
    use crate::interfaces::InterfaceId;
    use crate::wire::DestinationHash;

    const TEST_COMMAND_ID: CommandId = CommandId(7);

    fn announce_now(destination: DestinationHash) -> IssuedCommand {
        IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        }
    }

    #[test]
    fn an_announce_now_for_a_registered_single_owes_the_announce() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];

        assert_eq!(
            state.ingest_command(announce_now(destination), &[]),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Scheduled,
                },
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn an_announce_now_for_an_unknown_destination_is_rejected() {
        let mut state = personal_node_announcer();

        assert_eq!(
            state.ingest_command(announce_now(DestinationHash::new([0x77; 16])), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::UnknownDestination,
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn an_announce_now_for_a_plain_destination_is_rejected() {
        let mut state = personal_node_announcer();
        let plain = state
            .register_plain_destination("personal", &["plain"])
            .unwrap();

        assert_eq!(
            state.ingest_command(announce_now(plain), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::NotASingleDestination,
            },
        );
    }

    #[test]
    fn an_announce_now_targets_only_interfaces_the_view_offers() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let view = [routable_descriptor(InterfaceId::new([0xAA; 16]))];
        let on = |interface| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::Interface(interface),
                app_data: AnnounceAppData::Scheduled,
            }),
        };

        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xAA; 16])), &view),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::Interface(InterfaceId::new([0xAA; 16])),
                    app_data: AnnounceAppData::Scheduled,
                },
            },
        );
        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xBB; 16])), &view),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::UnknownInterface,
            },
        );
    }

    #[test]
    fn each_outcome_echoes_its_own_command_id() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let issued_as = |id| IssuedCommand {
            id,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        };

        for id in [CommandId(0), CommandId(42), CommandId(u64::MAX)] {
            assert_eq!(
                state.ingest_command(issued_as(id), &[]),
                CommandOutcome::OwesAnnounce {
                    id,
                    announce: AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Scheduled,
                    },
                },
            );
        }
    }

    #[test]
    fn announce_now_recovers_its_typed_settlement() {
        let verb = AnnounceNow {
            destination: DestinationHash::new([0x11; 16]),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };

        assert_eq!(
            verb.clone().into_command(),
            EngineCommand::AnnounceNow(verb),
        );
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Ok(()))),
            Some(Ok(())),
        );
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Err(
                AnnounceNowFailure::Rejected(AnnounceNowError::UnknownDestination)
            ))),
            Some(Err(AnnounceNowFailure::Rejected(
                AnnounceNowError::UnknownDestination
            ))),
        );
    }

    #[test]
    fn a_request_path_owes_its_emission_for_any_destination() {
        // No registration, no route — a path request to a wholly unknown
        // destination still owes its emission. That is the point of asking.
        let mut state = personal_node_announcer();
        let request = RequestPath {
            destination: DestinationHash::new([0x44; 16]),
            id: PathRequestId::new([0x55; 16]),
        };

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: TEST_COMMAND_ID,
                    command: EngineCommand::RequestPath(request),
                },
                &[],
            ),
            CommandOutcome::OwesPathRequest {
                id: TEST_COMMAND_ID,
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
            RequestPath::from_settlement(Settlement::RequestPath(Ok(()))),
            Some(Ok(())),
        );
        assert_eq!(
            RequestPath::from_settlement(Settlement::AnnounceNow(Ok(()))),
            None,
            "a path request never reads another verb's settlement",
        );
    }

    #[test]
    fn commanded_app_data_reserves_announce_room_for_the_ratchet() {
        let oversized =
            SelfAnnounceAppData::from_slice(&[0u8; MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN + 1])
                .unwrap();
        let with_data = |destination| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(oversized.clone()),
            }),
        };

        let mut ratcheted = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = ratcheted.self_announced_destinations()[0];
        assert_eq!(
            ratcheted.ingest_command(with_data(destination), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::AppDataTooLong,
            },
        );

        let mut unratcheted = personal_node_announcer();
        let destination = unratcheted.self_announced_destinations()[0];
        assert_eq!(
            unratcheted.ingest_command(with_data(destination), &[]),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Data(oversized),
                },
            },
        );
    }
}
