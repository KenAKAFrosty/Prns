//! App-issued commands, ingested by the engine as plain data.
//! Commands cross thread, task, and FFI boundaries as owned values,
//! so any host can queue them and the engine cycle drains them deterministically.
//!
//! RNS 1.3.1 has no scheduled announces at all: `Destination.announce()` is
//! app-called, and periodic announcing lives in app land (LXMF runs its own
//! timers). So [`AnnounceNow`] is the reference primitive, and this engine's
//! re-announce schedule is the extension built ahead of it.

use crate::engine::self_announce::SelfAnnounceAppData;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
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
    OwesAnnounce(AnnounceNow),
    AnnounceRejected(AnnounceNowError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowError {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    UnknownInterface,
}

use crate::engine::self_announce::MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN;
use crate::engine::EngineState;
use crate::routing::storage::EngineStorage;
use crate::wire::DestinationType;

impl<S: EngineStorage> EngineState<S> {
    #[must_use]
    pub fn ingest_command(&mut self, command: EngineCommand) -> CommandOutcome {
        self.ingested_command_count = self.ingested_command_count.saturating_add(1);
        match command {
            EngineCommand::AnnounceNow(announce_now) => self.ingest_announce_now(announce_now),
        }
    }

    fn ingest_announce_now(&self, announce_now: AnnounceNow) -> CommandOutcome {
        if self
            .upstream_app_destinations
            .lookup(&announce_now.destination, DestinationType::Single)
            .is_none()
        {
            return CommandOutcome::AnnounceRejected(
                if self
                    .upstream_app_destinations
                    .lookup(&announce_now.destination, DestinationType::Plain)
                    .is_some()
                {
                    AnnounceNowError::NotASingleDestination
                } else {
                    AnnounceNowError::UnknownDestination
                },
            );
        }
        if let AnnounceTarget::Interface(interface) = announce_now.target {
            if !self.interfaces.contains(&interface) {
                return CommandOutcome::AnnounceRejected(AnnounceNowError::UnknownInterface);
            }
        }
        if let AnnounceAppData::Data(data) = &announce_now.app_data {
            if self.self_ratchets.is_tracked(&announce_now.destination)
                && data.len() > MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN
            {
                return CommandOutcome::AnnounceRejected(AnnounceNowError::AppDataTooLong);
            }
        }
        CommandOutcome::OwesAnnounce(announce_now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::RatchetPolicy;
    use crate::interfaces::InterfaceId;
    use crate::wire::DestinationHash;

    fn announce_now(destination: DestinationHash) -> EngineCommand {
        EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        })
    }

    #[test]
    fn an_announce_now_for_a_registered_single_owes_the_announce() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];

        assert_eq!(
            state.ingest_command(announce_now(destination)),
            CommandOutcome::OwesAnnounce(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Scheduled,
            }),
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn an_announce_now_for_an_unknown_destination_is_rejected() {
        let mut state = personal_node_announcer();

        assert_eq!(
            state.ingest_command(announce_now(DestinationHash::new([0x77; 16]))),
            CommandOutcome::AnnounceRejected(AnnounceNowError::UnknownDestination),
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
            state.ingest_command(announce_now(plain)),
            CommandOutcome::AnnounceRejected(AnnounceNowError::NotASingleDestination),
        );
    }

    #[test]
    fn an_announce_now_targets_only_interfaces_the_engine_knows() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        register_test_interface(&mut state, InterfaceId::new([0xAA; 16]));
        let on = |interface| {
            EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::Interface(interface),
                app_data: AnnounceAppData::Scheduled,
            })
        };

        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xAA; 16]))),
            CommandOutcome::OwesAnnounce(AnnounceNow {
                destination,
                target: AnnounceTarget::Interface(InterfaceId::new([0xAA; 16])),
                app_data: AnnounceAppData::Scheduled,
            }),
        );
        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xBB; 16]))),
            CommandOutcome::AnnounceRejected(AnnounceNowError::UnknownInterface),
        );
    }

    #[test]
    fn commanded_app_data_reserves_announce_room_for_the_ratchet() {
        let oversized =
            SelfAnnounceAppData::from_slice(&[0u8; MAX_RATCHETED_SELF_ANNOUNCE_APP_DATA_LEN + 1])
                .unwrap();
        let with_data = |destination| {
            EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(oversized.clone()),
            })
        };

        let mut ratcheted = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = ratcheted.self_announced_destinations()[0];
        assert_eq!(
            ratcheted.ingest_command(with_data(destination)),
            CommandOutcome::AnnounceRejected(AnnounceNowError::AppDataTooLong),
        );

        let mut unratcheted = personal_node_announcer();
        let destination = unratcheted.self_announced_destinations()[0];
        assert_eq!(
            unratcheted.ingest_command(with_data(destination)),
            CommandOutcome::OwesAnnounce(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(oversized),
            }),
        );
    }
}
