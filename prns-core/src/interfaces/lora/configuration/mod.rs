use super::LoRaConfigurationState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareConfiguration {
    Confirmed(LoRaConfigurationState),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableConfiguration {
    Confirmed(LoRaConfigurationState),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationOperation {
    Quiesce,
    Apply(LoRaConfigurationState),
    Persist(LoRaConfigurationState),
    RestoreHardware(LoRaConfigurationState),
    RestoreStorage(LoRaConfigurationState),
    RestoreTraffic(LoRaConfigurationState),
    Publish(LoRaConfigurationState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationTicket {
    transaction: u64,
    operation: ConfigurationOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationEffect {
    pub ticket: ConfigurationTicket,
    pub operation: ConfigurationOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationCompletion {
    Succeeded,
    Failed,
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationFailure {
    Quiesce,
    Apply,
    Persist,
    Publish,
    RestoreHardware,
    RestoreStorage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationRefusal {
    Busy,
    RecoveryRequired,
    IdentityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationOutcome {
    Effect(ConfigurationEffect),
    Saved,
    Failed(ConfigurationFailure),
    Refused {
        reason: ConfigurationRefusal,
        requested: LoRaConfigurationState,
    },
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transaction {
    previous: LoRaConfigurationState,
    requested: LoRaConfigurationState,
    failure: ConfigurationFailure,
    storage: StorageRestoration,
    effect: ConfigurationEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorageRestoration {
    Required,
    Unnecessary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigurationPhase {
    Settled,
    Changing(Transaction),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfigurationMachine {
    hardware: HardwareConfiguration,
    durable: DurableConfiguration,
    next_transaction: u64,
    phase: ConfigurationPhase,
}

impl ConfigurationMachine {
    pub const fn new(configuration: LoRaConfigurationState) -> Self {
        Self {
            hardware: HardwareConfiguration::Confirmed(configuration),
            durable: DurableConfiguration::Confirmed(configuration),
            next_transaction: 0,
            phase: ConfigurationPhase::Settled,
        }
    }
    pub const fn unresolved() -> Self {
        Self {
            hardware: HardwareConfiguration::Unknown,
            durable: DurableConfiguration::Unknown,
            next_transaction: 0,
            phase: ConfigurationPhase::Settled,
        }
    }

    pub const fn hardware(&self) -> HardwareConfiguration {
        self.hardware
    }
    pub const fn durable(&self) -> DurableConfiguration {
        self.durable
    }
    pub const fn is_settled(&self) -> bool {
        matches!(self.phase, ConfigurationPhase::Settled)
    }

    pub fn begin(&mut self, requested: LoRaConfigurationState) -> ConfigurationOutcome {
        if let ConfigurationPhase::Changing(_) = self.phase {
            return ConfigurationOutcome::Refused {
                reason: ConfigurationRefusal::Busy,
                requested,
            };
        }
        let (HardwareConfiguration::Confirmed(previous), DurableConfiguration::Confirmed(saved)) =
            (self.hardware, self.durable)
        else {
            return ConfigurationOutcome::Refused {
                reason: ConfigurationRefusal::RecoveryRequired,
                requested,
            };
        };
        if previous != saved {
            return ConfigurationOutcome::Refused {
                reason: ConfigurationRefusal::RecoveryRequired,
                requested,
            };
        }
        if requested == previous {
            return ConfigurationOutcome::Saved;
        }
        let Some(transaction) = self.next_transaction.checked_add(1) else {
            return ConfigurationOutcome::Refused {
                reason: ConfigurationRefusal::IdentityExhausted,
                requested,
            };
        };
        self.next_transaction = transaction;
        let operation = ConfigurationOperation::Quiesce;
        let effect = ConfigurationEffect {
            ticket: ConfigurationTicket {
                transaction,
                operation,
            },
            operation,
        };
        self.phase = ConfigurationPhase::Changing(Transaction {
            previous,
            requested,
            failure: ConfigurationFailure::Apply,
            storage: StorageRestoration::Unnecessary,
            effect,
        });
        ConfigurationOutcome::Effect(effect)
    }

    pub fn complete(
        &mut self,
        ticket: ConfigurationTicket,
        completion: ConfigurationCompletion,
    ) -> ConfigurationOutcome {
        let ConfigurationPhase::Changing(mut transaction) = self.phase else {
            return ConfigurationOutcome::Stale;
        };
        if ticket != transaction.effect.ticket {
            return ConfigurationOutcome::Stale;
        }
        let next = match (ticket.operation, completion) {
            (ConfigurationOperation::Quiesce, ConfigurationCompletion::Succeeded) => {
                ConfigurationOperation::Apply(transaction.requested)
            }
            (
                ConfigurationOperation::Quiesce,
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                self.hardware = HardwareConfiguration::Unknown;
                transaction.failure = ConfigurationFailure::Quiesce;
                ConfigurationOperation::RestoreHardware(transaction.previous)
            }
            (ConfigurationOperation::Apply(configuration), ConfigurationCompletion::Succeeded) => {
                self.hardware = HardwareConfiguration::Confirmed(configuration);
                ConfigurationOperation::Persist(configuration)
            }
            (
                ConfigurationOperation::Apply(_),
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                self.hardware = HardwareConfiguration::Unknown;
                transaction.failure = ConfigurationFailure::Apply;
                ConfigurationOperation::RestoreHardware(transaction.previous)
            }
            (
                ConfigurationOperation::Persist(configuration),
                ConfigurationCompletion::Succeeded,
            ) => {
                self.durable = DurableConfiguration::Confirmed(configuration);
                ConfigurationOperation::Publish(configuration)
            }
            (ConfigurationOperation::Persist(_), ConfigurationCompletion::Failed) => {
                transaction.failure = ConfigurationFailure::Persist;
                ConfigurationOperation::RestoreHardware(transaction.previous)
            }
            (ConfigurationOperation::Persist(_), ConfigurationCompletion::Indeterminate) => {
                self.durable = DurableConfiguration::Unknown;
                transaction.storage = StorageRestoration::Required;
                transaction.failure = ConfigurationFailure::Persist;
                ConfigurationOperation::RestoreHardware(transaction.previous)
            }
            (ConfigurationOperation::Publish(_), ConfigurationCompletion::Succeeded) => {
                self.phase = ConfigurationPhase::Settled;
                return ConfigurationOutcome::Saved;
            }
            (
                ConfigurationOperation::Publish(_),
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                transaction.failure = ConfigurationFailure::Publish;
                transaction.storage = StorageRestoration::Required;
                ConfigurationOperation::RestoreHardware(transaction.previous)
            }
            (
                ConfigurationOperation::RestoreHardware(configuration),
                ConfigurationCompletion::Succeeded,
            ) => {
                self.hardware = HardwareConfiguration::Confirmed(configuration);
                match transaction.storage {
                    StorageRestoration::Required => {
                        ConfigurationOperation::RestoreStorage(configuration)
                    }
                    StorageRestoration::Unnecessary => {
                        ConfigurationOperation::RestoreTraffic(configuration)
                    }
                }
            }
            (
                ConfigurationOperation::RestoreHardware(_),
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                self.hardware = HardwareConfiguration::Unknown;
                transaction.failure = ConfigurationFailure::RestoreHardware;
                match transaction.storage {
                    StorageRestoration::Required => {
                        ConfigurationOperation::RestoreStorage(transaction.previous)
                    }
                    StorageRestoration::Unnecessary => {
                        self.phase = ConfigurationPhase::Settled;
                        return ConfigurationOutcome::Failed(ConfigurationFailure::RestoreHardware);
                    }
                }
            }
            (
                ConfigurationOperation::RestoreStorage(configuration),
                ConfigurationCompletion::Succeeded,
            ) => {
                self.durable = DurableConfiguration::Confirmed(configuration);
                if self.hardware == HardwareConfiguration::Unknown {
                    self.phase = ConfigurationPhase::Settled;
                    return ConfigurationOutcome::Failed(transaction.failure);
                }
                ConfigurationOperation::RestoreTraffic(configuration)
            }
            (ConfigurationOperation::RestoreTraffic(_), ConfigurationCompletion::Succeeded) => {
                self.phase = ConfigurationPhase::Settled;
                return ConfigurationOutcome::Failed(transaction.failure);
            }
            (
                ConfigurationOperation::RestoreTraffic(_),
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                self.hardware = HardwareConfiguration::Unknown;
                self.phase = ConfigurationPhase::Settled;
                return ConfigurationOutcome::Failed(ConfigurationFailure::RestoreHardware);
            }
            (
                ConfigurationOperation::RestoreStorage(_),
                ConfigurationCompletion::Failed | ConfigurationCompletion::Indeterminate,
            ) => {
                self.durable = DurableConfiguration::Unknown;
                self.phase = ConfigurationPhase::Settled;
                return ConfigurationOutcome::Failed(ConfigurationFailure::RestoreStorage);
            }
        };
        let effect = ConfigurationEffect {
            ticket: ConfigurationTicket {
                transaction: ticket.transaction,
                operation: next,
            },
            operation: next,
        };
        transaction.effect = effect;
        self.phase = ConfigurationPhase::Changing(transaction);
        ConfigurationOutcome::Effect(effect)
    }
}

#[cfg(kani)]
mod kani_proofs;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioConfigurationOperation {
    Quiesce,
    Stage(LoRaConfigurationState),
    Publish,
    Resume,
    Hold,
}

#[allow(async_fn_in_trait)]
pub trait ConfigurationRadio {
    async fn perform(&mut self, operation: RadioConfigurationOperation) -> ConfigurationCompletion;
}
