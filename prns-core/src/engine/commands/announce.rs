use crate::engine::{AnnounceWriteFailure, EngineState};
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::InterfaceId;
use crate::routing::announce::emit::{AnnounceAppDataBytes, MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN};
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::{CommandId, CommandOutcome, PrnsCommand, Settleable, Settlement};

/// `Destination.announce(app_data=…, attached_interface=…)` as data (RNS 1.4.2 Destination.py).
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

// Data rides the announce's full app-data capacity inline beside the zero-size registered.
// The skew is the point of the pair, and the no-alloc core has no Box to shrink it.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnounceAppData {
    Registered,
    Data(AnnounceAppDataBytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowRejection {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    UnknownInterface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowFailure {
    Rejected(AnnounceNowRejection),
    WriteFailed(AnnounceWriteFailure),
}

impl Settleable for AnnounceNow {
    type Success = ();
    type Failure = AnnounceNowFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::AnnounceNow(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), AnnounceNowFailure>> {
        match settlement {
            Settlement::AnnounceNow(result) => Some(result),

            Settlement::SendSinglePacket(_)
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
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_announce_now(
        &self,
        id: CommandId,
        announce_now: AnnounceNow,
        interfaces: AttachedInterfaces<'_>,
    ) -> CommandOutcome {
        match self
            .upstream_app_destinations
            .registration_for(&announce_now.destination)
        {
            None => {
                return CommandOutcome::AnnounceRejected {
                    id,
                    rejection: AnnounceNowRejection::UnknownDestination,
                };
            }
            Some((registered, _)) => {
                if !matches!(registered.kind, UpstreamAppDestinationKind::Single { .. }) {
                    return CommandOutcome::AnnounceRejected {
                        id,
                        rejection: AnnounceNowRejection::NotASingleDestination,
                    };
                }
            }
        }
        if let AnnounceTarget::Interface(interface) = announce_now.target {
            if !interfaces
                .iter()
                .any(|descriptor| descriptor.id == interface)
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    rejection: AnnounceNowRejection::UnknownInterface,
                };
            }
        }
        if let AnnounceAppData::Data(data) = &announce_now.app_data {
            // Only the ratcheted tightening needs a runtime gate.
            // The type's capacity IS the unratcheted maximum, and ratcheting was this node's own registration-time RatchetPolicy choice, which spends RATCHET_BYTE_LEN of the app-data budget on the wire.
            // Registration enforces the same bound for Registered app data.
            if self.self_ratchets.is_tracked(&announce_now.destination)
                && data.len() > MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    rejection: AnnounceNowRejection::AppDataTooLong,
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
    use crate::engine::{EngineReaction, InstantMillis, IssuedCommand, Journaled, RatchetPolicy};

    const TEST_COMMAND_ID: CommandId = CommandId(7);

    fn announce_now(destination: DestinationHash) -> IssuedCommand {
        IssuedCommand {
            id: TEST_COMMAND_ID,
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }
    }

    #[test]
    fn an_announce_now_for_a_registered_single_owes_the_announce() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();

        assert_eq!(
            state.ingest_command(announce_now(destination), AttachedInterfaces::new(&[])),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn a_ratcheted_announce_journals_its_rotation_before_settlement() {
        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = personal_node_destination();
        let mut journal = std::vec::Vec::new();

        let _ = state.ingest_command_into(
            announce_now(destination),
            AttachedInterfaces::new(&[]),
            InstantMillis(1_000),
            &mut test_fill_entropy,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::SelfRatchetRotated { destination }) => {
                    journal.push((destination, None));
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    assert_eq!(id, TEST_COMMAND_ID);
                    journal.push((destination, Some(settlement)));
                }
                _ => {}
            },
        );

        assert_eq!(
            journal,
            std::vec![
                (destination, None),
                (destination, Some(Settlement::AnnounceNow(Ok(())))),
            ],
        );
    }

    #[test]
    fn an_announce_now_for_an_unknown_destination_is_rejected() {
        let mut state = personal_node_announcer();

        assert_eq!(
            state.ingest_command(
                announce_now(DestinationHash::new([0x77; 16])),
                AttachedInterfaces::new(&[])
            ),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                rejection: AnnounceNowRejection::UnknownDestination,
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
            state.ingest_command(announce_now(plain), AttachedInterfaces::new(&[])),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                rejection: AnnounceNowRejection::NotASingleDestination,
            },
        );
    }

    #[test]
    fn an_announce_now_for_a_group_destination_is_rejected_as_not_single() {
        use crate::identity::IdentityHash;

        let mut state = personal_node_announcer();
        let group = state
            .register_group_destination(
                &IdentityHash::new([0x33; 16]),
                "personal",
                &["group"],
                &[0xAB; 32],
            )
            .unwrap();

        assert_eq!(
            state.ingest_command(announce_now(group), AttachedInterfaces::new(&[])),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                rejection: AnnounceNowRejection::NotASingleDestination,
            },
        );
    }

    #[test]
    fn an_announce_now_targets_only_interfaces_the_view_offers() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let interfaces = [routable_descriptor(InterfaceId::new([0xAA; 8]))];
        let on = |interface| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::Interface(interface),
                app_data: AnnounceAppData::Registered,
            }),
        };

        assert_eq!(
            state.ingest_command(
                on(InterfaceId::new([0xAA; 8])),
                AttachedInterfaces::new(&interfaces)
            ),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::Interface(InterfaceId::new([0xAA; 8])),
                    app_data: AnnounceAppData::Registered,
                },
            },
        );
        assert_eq!(
            state.ingest_command(
                on(InterfaceId::new([0xBB; 8])),
                AttachedInterfaces::new(&interfaces)
            ),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                rejection: AnnounceNowRejection::UnknownInterface,
            },
        );
    }

    #[test]
    fn announce_now_recovers_its_typed_settlement() {
        let verb = AnnounceNow {
            destination: DestinationHash::new([0x11; 16]),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };

        assert_eq!(verb.clone().into_command(), PrnsCommand::AnnounceNow(verb),);
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Ok(()))),
            Some(Ok(())),
        );
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Err(
                AnnounceNowFailure::Rejected(AnnounceNowRejection::UnknownDestination)
            ))),
            Some(Err(AnnounceNowFailure::Rejected(
                AnnounceNowRejection::UnknownDestination
            ))),
        );
    }

    #[test]
    fn commanded_app_data_reserves_announce_room_for_the_ratchet() {
        let oversized =
            AnnounceAppDataBytes::from_slice(&[0u8; MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN + 1])
                .unwrap();
        let with_data = |destination| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(oversized.clone()),
            }),
        };

        let mut ratcheted = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = personal_node_destination();
        assert_eq!(
            ratcheted.ingest_command(with_data(destination), AttachedInterfaces::new(&[])),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                rejection: AnnounceNowRejection::AppDataTooLong,
            },
        );

        let mut unratcheted = personal_node_announcer();
        let destination = personal_node_destination();
        assert_eq!(
            unratcheted.ingest_command(with_data(destination), AttachedInterfaces::new(&[])),
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
