use crate::crypto::ratchets::RatchetPolicy;
use crate::engine::EngineState;
use crate::routing::announce::emit::{AnnounceAppDataBytes, MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN};
use crate::routing::upstream_app_destinations::UpstreamAppDestinationKind;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::{CommandId, CommandOutcome, PrnsCommand, Settleable, Settlement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetRegisteredAnnounceAppData {
    pub destination: DestinationHash,
    pub app_data: AnnounceAppDataBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRegisteredAnnounceAppDataRejection {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRegisteredAnnounceAppDataFailure {
    Rejected(SetRegisteredAnnounceAppDataRejection),
}

impl Settleable for SetRegisteredAnnounceAppData {
    type Success = ();
    type Failure = SetRegisteredAnnounceAppDataFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SetRegisteredAnnounceAppData(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<(), SetRegisteredAnnounceAppDataFailure>> {
        match settlement {
            Settlement::SetRegisteredAnnounceAppData(result) => Some(result),

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
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_set_registered_announce_app_data(
        &mut self,
        id: CommandId,
        set: SetRegisteredAnnounceAppData,
    ) -> CommandOutcome {
        let Some((registered, _)) = self
            .upstream_app_destinations
            .registration_for(&set.destination)
        else {
            return CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                id,
                rejection: SetRegisteredAnnounceAppDataRejection::UnknownDestination,
            };
        };
        let UpstreamAppDestinationKind::Single { ratchet_policy, .. } = registered.kind else {
            return CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                id,
                rejection: SetRegisteredAnnounceAppDataRejection::NotASingleDestination,
            };
        };
        if matches!(
            ratchet_policy,
            RatchetPolicy::Ratcheted | RatchetPolicy::RatchetsRequired
        ) && set.app_data.len() > MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN
        {
            return CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                id,
                rejection: SetRegisteredAnnounceAppDataRejection::AppDataTooLong,
            };
        }
        let replaced = self
            .upstream_app_destinations
            .replace_registered_announce_app_data(&set.destination, set.app_data);
        match replaced {
            Some(_) => CommandOutcome::RegisteredAnnounceAppDataSet { id },
            None => CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                id,
                rejection: SetRegisteredAnnounceAppDataRejection::UnknownDestination,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{personal_node_announcer, personal_node_announcer_with};
    use crate::engine::{IssuedCommand, RatchetPolicy};
    use crate::identity::IdentityHash;
    use crate::interfaces::AttachedInterfaces;

    const TEST_COMMAND_ID: CommandId = CommandId(7);

    fn set(destination: DestinationHash, app_data: AnnounceAppDataBytes) -> IssuedCommand {
        IssuedCommand {
            id: TEST_COMMAND_ID,
            command: PrnsCommand::SetRegisteredAnnounceAppData(SetRegisteredAnnounceAppData {
                destination,
                app_data,
            }),
        }
    }

    #[test]
    fn a_registered_single_replaces_its_default_announcement_data() {
        let mut state = personal_node_announcer();
        let destination = crate::engine::test_support::personal_node_destination();
        let app_data = AnnounceAppDataBytes::from_slice(b"new default").unwrap();

        assert_eq!(
            state.ingest_command(set(destination, app_data), AttachedInterfaces::new(&[])),
            CommandOutcome::RegisteredAnnounceAppDataSet {
                id: TEST_COMMAND_ID,
            },
        );
        assert_eq!(
            state.upstream_app_destinations.app_data_for(&destination),
            Some(b"new default".as_slice()),
        );
    }

    #[test]
    fn an_unknown_or_non_single_destination_is_rejected_without_mutation() {
        let mut state = personal_node_announcer();
        let unknown = DestinationHash::new([0x77; 16]);
        let plain = state
            .register_plain_destination("application", &["plain"])
            .unwrap();
        let group = state
            .register_group_destination(
                &IdentityHash::new([0x33; 16]),
                "application",
                &["group"],
                &[0x44; 32],
            )
            .unwrap();

        for (destination, rejection) in [
            (
                unknown,
                SetRegisteredAnnounceAppDataRejection::UnknownDestination,
            ),
            (
                plain,
                SetRegisteredAnnounceAppDataRejection::NotASingleDestination,
            ),
            (
                group,
                SetRegisteredAnnounceAppDataRejection::NotASingleDestination,
            ),
        ] {
            assert_eq!(
                state.ingest_command(
                    set(
                        destination,
                        AnnounceAppDataBytes::from_slice(b"refused").unwrap(),
                    ),
                    AttachedInterfaces::new(&[]),
                ),
                CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                    id: TEST_COMMAND_ID,
                    rejection,
                },
            );
        }
        assert_eq!(
            state.upstream_app_destinations.app_data_for(&plain),
            Some([].as_slice()),
        );
        assert_eq!(
            state.upstream_app_destinations.app_data_for(&group),
            Some([].as_slice()),
        );
    }

    #[test]
    fn ratcheted_destinations_enforce_their_smaller_registered_data_budget() {
        let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = crate::engine::test_support::personal_node_destination();
        let maximum = [0xA5; MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN];
        let oversized = [0x5A; MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN.saturating_add(1)];

        assert_eq!(
            state.ingest_command(
                set(
                    destination,
                    AnnounceAppDataBytes::from_slice(oversized.as_slice()).unwrap(),
                ),
                AttachedInterfaces::new(&[]),
            ),
            CommandOutcome::SetRegisteredAnnounceAppDataRejected {
                id: TEST_COMMAND_ID,
                rejection: SetRegisteredAnnounceAppDataRejection::AppDataTooLong,
            },
        );
        assert_eq!(
            state.ingest_command(
                set(
                    destination,
                    AnnounceAppDataBytes::from_slice(maximum.as_slice()).unwrap(),
                ),
                AttachedInterfaces::new(&[]),
            ),
            CommandOutcome::RegisteredAnnounceAppDataSet {
                id: TEST_COMMAND_ID,
            },
        );
        assert_eq!(
            state.upstream_app_destinations.app_data_for(&destination),
            Some(maximum.as_slice()),
        );
    }

    #[test]
    fn the_typed_command_recovers_only_its_own_settlement() {
        assert_eq!(
            SetRegisteredAnnounceAppData::from_settlement(
                Settlement::SetRegisteredAnnounceAppData(Ok(())),
            ),
            Some(Ok(())),
        );
        assert_eq!(
            SetRegisteredAnnounceAppData::from_settlement(Settlement::AnnounceNow(Ok(()))),
            None,
        );
    }
}
