use crate::{SubGConfigurationCommitOutcome, SubGConfigurationStore};
use embedded_storage_async::nor_flash::NorFlash;
use personal_rns::interfaces::lora::{configuration::*, LoRaConfigurationState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfigurationResult {
    Saved,
    Failed(ConfigurationFailure),
    Refused(ConfigurationRefusal),
    StaleCompletion,
}

pub struct LoRaConfigurationService {
    machine: ConfigurationMachine,
}

impl LoRaConfigurationService {
    pub const fn new(configuration: LoRaConfigurationState) -> Self {
        Self {
            machine: ConfigurationMachine::new(configuration),
        }
    }

    pub const fn unresolved() -> Self {
        Self {
            machine: ConfigurationMachine::unresolved(),
        }
    }

    pub const fn durable(&self) -> DurableConfiguration {
        self.machine.durable()
    }
    pub const fn hardware(&self) -> HardwareConfiguration {
        self.machine.hardware()
    }

    pub fn operating_state(
        &self,
        observed: personal_rns::interfaces::ConnectionState,
    ) -> personal_rns::remote_control::RemoteControlRadioOperatingState {
        use personal_rns::interfaces::ConnectionState as Connection;
        use personal_rns::remote_control::RemoteControlRadioOperatingState as Operating;
        if !self.machine.is_settled() {
            return Operating::Changing;
        }
        let HardwareConfiguration::Confirmed(configuration) = self.hardware() else {
            return Operating::Failed;
        };
        if self.durable() == DurableConfiguration::Unknown {
            return Operating::Failed;
        }
        match observed {
            Connection::Initializing | Connection::Reconnecting => Operating::Changing,
            Connection::Failed
            | Connection::Unknown
            | Connection::Disconnected
            | Connection::Degraded => Operating::Failed,
            Connection::Disabled => match configuration {
                LoRaConfigurationState::Unconfigured => Operating::Unconfigured,
                LoRaConfigurationState::Configured(_) => Operating::Disabled,
            },
            Connection::Connected => match configuration {
                LoRaConfigurationState::Unconfigured => Operating::Failed,
                LoRaConfigurationState::Configured(_) => Operating::Operating,
            },
        }
    }

    /// The host command owner awaits this operation even if its requester
    /// disconnects. No network request future owns the admitted transaction.
    pub async fn apply<F: NorFlash, R: ConfigurationRadio>(
        &mut self,
        requested: LoRaConfigurationState,
        radio: &mut R,
        store: &mut SubGConfigurationStore<F>,
    ) -> LoRaConfigurationResult {
        let mut outcome = self.machine.begin(requested);
        loop {
            outcome = match outcome {
                ConfigurationOutcome::Effect(effect) => {
                    let completion = match effect.operation {
                        ConfigurationOperation::Quiesce => {
                            radio.perform(RadioConfigurationOperation::Quiesce).await
                        }
                        ConfigurationOperation::Apply(configuration)
                        | ConfigurationOperation::RestoreHardware(configuration) => {
                            radio
                                .perform(RadioConfigurationOperation::Stage(configuration))
                                .await
                        }
                        ConfigurationOperation::Publish(_) => {
                            radio.perform(RadioConfigurationOperation::Publish).await
                        }
                        ConfigurationOperation::RestoreTraffic(_) => {
                            radio.perform(RadioConfigurationOperation::Resume).await
                        }
                        ConfigurationOperation::Persist(configuration)
                        | ConfigurationOperation::RestoreStorage(configuration) => {
                            match store.save_radio(configuration).await {
                                SubGConfigurationCommitOutcome::Committed => {
                                    ConfigurationCompletion::Succeeded
                                }
                                SubGConfigurationCommitOutcome::NotCommitted(_) => {
                                    ConfigurationCompletion::Failed
                                }
                                SubGConfigurationCommitOutcome::Indeterminate(_) => {
                                    ConfigurationCompletion::Indeterminate
                                }
                            }
                        }
                    };
                    self.machine.complete(effect.ticket, completion)
                }
                ConfigurationOutcome::Saved => return LoRaConfigurationResult::Saved,
                ConfigurationOutcome::Failed(failure) => {
                    if self.machine.hardware() == HardwareConfiguration::Unknown
                        || self.machine.durable() == DurableConfiguration::Unknown
                    {
                        let _ = radio.perform(RadioConfigurationOperation::Hold).await;
                    }
                    return LoRaConfigurationResult::Failed(failure);
                }
                ConfigurationOutcome::Refused { reason, .. } => {
                    return LoRaConfigurationResult::Refused(reason)
                }
                ConfigurationOutcome::Stale => return LoRaConfigurationResult::StaleCompletion,
            };
        }
    }
}

impl LoRaConfigurationResult {
    pub const fn remote_outcome(self) -> personal_rns::remote_control::RemoteControlRadioOutcome {
        use personal_rns::remote_control::RemoteControlRadioOutcome as Outcome;
        match self {
            Self::Saved => Outcome::Saved,
            Self::Failed(ConfigurationFailure::Quiesce | ConfigurationFailure::Apply) => {
                Outcome::HardwareFailed
            }
            Self::Failed(ConfigurationFailure::Persist) => Outcome::PersistenceFailed,
            Self::Failed(ConfigurationFailure::Publish) => Outcome::PublicationFailed,
            Self::Failed(
                ConfigurationFailure::RestoreHardware | ConfigurationFailure::RestoreStorage,
            )
            | Self::StaleCompletion => Outcome::RecoveryRequired,
            Self::Refused(ConfigurationRefusal::Busy) => Outcome::Busy,
            Self::Refused(ConfigurationRefusal::RecoveryRequired) => Outcome::RecoveryRequired,
            Self::Refused(ConfigurationRefusal::IdentityExhausted) => Outcome::IdentityExhausted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::remote_control::RemoteControlRadioOutcome as Remote;

    #[test]
    fn unresolved_startup_keeps_both_owners_unknown() {
        let service = LoRaConfigurationService::unresolved();
        assert_eq!(service.hardware(), HardwareConfiguration::Unknown);
        assert_eq!(service.durable(), DurableConfiguration::Unknown);
    }

    #[test]
    fn every_transaction_result_has_an_explicit_remote_outcome() {
        use ConfigurationFailure as Failure;
        use ConfigurationRefusal as Refusal;
        use LoRaConfigurationResult as Result;
        for (local, remote) in [
            (Result::Saved, Remote::Saved),
            (Result::Failed(Failure::Quiesce), Remote::HardwareFailed),
            (Result::Failed(Failure::Apply), Remote::HardwareFailed),
            (Result::Failed(Failure::Persist), Remote::PersistenceFailed),
            (Result::Failed(Failure::Publish), Remote::PublicationFailed),
            (
                Result::Failed(Failure::RestoreHardware),
                Remote::RecoveryRequired,
            ),
            (
                Result::Failed(Failure::RestoreStorage),
                Remote::RecoveryRequired,
            ),
            (Result::StaleCompletion, Remote::RecoveryRequired),
            (Result::Refused(Refusal::Busy), Remote::Busy),
            (
                Result::Refused(Refusal::RecoveryRequired),
                Remote::RecoveryRequired,
            ),
            (
                Result::Refused(Refusal::IdentityExhausted),
                Remote::IdentityExhausted,
            ),
        ] {
            assert_eq!(local.remote_outcome(), remote);
        }
    }
    #[test]
    fn operating_status_uses_observed_hardware_and_keeps_unknown_storage_failed() {
        use personal_rns::interfaces::lora::LoRaConfiguration;
        use personal_rns::interfaces::subghz::regions::us915::Us915;
        use personal_rns::interfaces::ConnectionState as Connection;
        use personal_rns::remote_control::RemoteControlRadioOperatingState as Operating;
        let requested =
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()));
        for initial in [LoRaConfigurationState::Unconfigured, requested] {
            let service = LoRaConfigurationService::new(initial);
            for (observed, expected) in [
                (Connection::Initializing, Operating::Changing),
                (Connection::Reconnecting, Operating::Changing),
                (Connection::Failed, Operating::Failed),
                (Connection::Unknown, Operating::Failed),
                (Connection::Disconnected, Operating::Failed),
                (Connection::Degraded, Operating::Failed),
                (
                    Connection::Disabled,
                    if initial == requested {
                        Operating::Disabled
                    } else {
                        Operating::Unconfigured
                    },
                ),
                (
                    Connection::Connected,
                    if initial == requested {
                        Operating::Operating
                    } else {
                        Operating::Failed
                    },
                ),
            ] {
                assert_eq!(service.operating_state(observed), expected);
            }
        }
        assert_eq!(
            LoRaConfigurationService::unresolved().operating_state(Connection::Disabled),
            Operating::Failed
        );
        let mut service = LoRaConfigurationService::new(LoRaConfigurationState::Unconfigured);
        let mut outcome = service.machine.begin(requested);
        assert_eq!(
            service.operating_state(Connection::Connected),
            Operating::Changing
        );
        while let ConfigurationOutcome::Effect(effect) = outcome {
            let completion = match effect.operation {
                ConfigurationOperation::Persist(_) => ConfigurationCompletion::Indeterminate,
                ConfigurationOperation::RestoreStorage(_) => ConfigurationCompletion::Failed,
                _ => ConfigurationCompletion::Succeeded,
            };
            outcome = service.machine.complete(effect.ticket, completion);
        }
        assert_eq!(
            service.hardware(),
            HardwareConfiguration::Confirmed(LoRaConfigurationState::Unconfigured)
        );
        assert_eq!(service.durable(), DurableConfiguration::Unknown);
        assert_eq!(
            service.operating_state(Connection::Disabled),
            Operating::Failed
        );
    }
}
