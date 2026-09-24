use super::*;
use crate::interfaces::lora::LoRaConfiguration;
use crate::interfaces::subghz::regions::us915::Us915;

fn configured() -> LoRaConfigurationState {
    LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()))
}
fn effect(outcome: ConfigurationOutcome) -> ConfigurationEffect {
    match outcome {
        ConfigurationOutcome::Effect(effect) => effect,
        other => panic!("expected effect: {other:?}"),
    }
}

#[test]
fn successful_change_has_ordered_effects_and_stale_completions_have_no_effect() {
    let previous = LoRaConfigurationState::Unconfigured;
    let requested = configured();
    let mut machine = ConfigurationMachine::new(previous);
    assert!(machine.is_settled());
    let first = effect(machine.begin(requested));
    assert!(!machine.is_settled());
    assert_eq!(
        machine.begin(previous),
        ConfigurationOutcome::Refused {
            reason: ConfigurationRefusal::Busy,
            requested: previous
        }
    );
    let mut pending = first;
    for operation in [
        ConfigurationOperation::Quiesce,
        ConfigurationOperation::Apply(requested),
        ConfigurationOperation::Persist(requested),
        ConfigurationOperation::Publish(requested),
    ] {
        assert_eq!(pending.operation, operation);
        let result = machine.complete(pending.ticket, ConfigurationCompletion::Succeeded);
        assert_eq!(
            machine.complete(pending.ticket, ConfigurationCompletion::Succeeded),
            ConfigurationOutcome::Stale
        );
        match result {
            ConfigurationOutcome::Effect(next) => pending = next,
            ConfigurationOutcome::Saved => {
                assert_eq!(operation, ConfigurationOperation::Publish(requested))
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(machine.is_settled());
    assert_eq!(
        machine.hardware(),
        HardwareConfiguration::Confirmed(requested)
    );
    assert_eq!(
        machine.durable(),
        DurableConfiguration::Confirmed(requested)
    );
    assert_eq!(machine.begin(requested), ConfigurationOutcome::Saved);
}

fn explore(mut machine: ConfigurationMachine, pending: ConfigurationEffect, depth: usize) {
    assert!(depth < 7);
    for completion in [
        ConfigurationCompletion::Succeeded,
        ConfigurationCompletion::Failed,
        ConfigurationCompletion::Indeterminate,
    ] {
        let mut candidate = ConfigurationMachine {
            hardware: machine.hardware,
            durable: machine.durable,
            next_transaction: machine.next_transaction,
            phase: machine.phase,
        };
        let outcome = candidate.complete(pending.ticket, completion);
        assert_eq!(
            candidate.complete(pending.ticket, completion),
            ConfigurationOutcome::Stale
        );
        match outcome {
            ConfigurationOutcome::Effect(next) => explore(candidate, next, depth + 1),
            ConfigurationOutcome::Saved => {
                assert_eq!(
                    candidate.hardware(),
                    HardwareConfiguration::Confirmed(configured())
                );
                assert_eq!(
                    candidate.durable(),
                    DurableConfiguration::Confirmed(configured())
                );
                assert!(candidate.is_settled());
            }
            ConfigurationOutcome::Failed(_) => {
                assert!(candidate.is_settled());
                let next = candidate.begin(configured());
                if candidate.hardware == HardwareConfiguration::Unknown
                    || candidate.durable == DurableConfiguration::Unknown
                {
                    assert_eq!(
                        next,
                        ConfigurationOutcome::Refused {
                            reason: ConfigurationRefusal::RecoveryRequired,
                            requested: configured()
                        }
                    );
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    // Beginning another request cannot replace the transaction being explored.
    assert_eq!(
        machine.begin(configured()),
        ConfigurationOutcome::Refused {
            reason: ConfigurationRefusal::Busy,
            requested: configured()
        }
    );
}

#[test]
fn every_fault_at_every_transaction_phase_settles_or_requests_bounded_recovery() {
    let mut machine = ConfigurationMachine::new(LoRaConfigurationState::Unconfigured);
    let pending = effect(machine.begin(configured()));
    explore(machine, pending, 0);
}

#[test]
fn exhausted_identity_and_inconsistent_confirmed_state_are_refused() {
    let mut machine = ConfigurationMachine::new(LoRaConfigurationState::Unconfigured);
    machine.next_transaction = u64::MAX;
    assert_eq!(
        machine.begin(configured()),
        ConfigurationOutcome::Refused {
            reason: ConfigurationRefusal::IdentityExhausted,
            requested: configured()
        }
    );
    machine.durable = DurableConfiguration::Confirmed(configured());
    assert_eq!(
        machine.begin(configured()),
        ConfigurationOutcome::Refused {
            reason: ConfigurationRefusal::RecoveryRequired,
            requested: configured()
        }
    );
}

proptest::proptest! {
    #[test]
    fn arbitrary_completions_cannot_publish_unsaved_settings(choices in proptest::collection::vec(0u8..3, 0..32)) {
        let mut machine = ConfigurationMachine::new(LoRaConfigurationState::Unconfigured);
        let mut pending = machine.begin(configured());
        for choice in choices {
            if let ConfigurationOutcome::Effect(effect) = pending {
                let result = [ConfigurationCompletion::Succeeded, ConfigurationCompletion::Failed, ConfigurationCompletion::Indeterminate][usize::from(choice)];
                pending = machine.complete(effect.ticket, result);
                if pending == ConfigurationOutcome::Saved {
                    proptest::prop_assert_eq!(machine.hardware(), HardwareConfiguration::Confirmed(configured()));
                    proptest::prop_assert_eq!(machine.durable(), DurableConfiguration::Confirmed(configured()));
                }
            }
        }
    }
}

#[test]
fn unresolved_startup_refuses_changes_without_claiming_known_storage_or_hardware() {
    let mut machine = ConfigurationMachine::unresolved();
    assert!(machine.is_settled());
    assert_eq!(machine.hardware(), HardwareConfiguration::Unknown);
    assert_eq!(machine.durable(), DurableConfiguration::Unknown);
    assert_eq!(
        machine.begin(configured()),
        ConfigurationOutcome::Refused {
            reason: ConfigurationRefusal::RecoveryRequired,
            requested: configured(),
        }
    );
}

#[test]
fn storage_restoration_resumes_traffic_only_after_hardware_was_restored() {
    let previous = LoRaConfigurationState::Unconfigured;
    for hardware_result in [
        ConfigurationCompletion::Succeeded,
        ConfigurationCompletion::Failed,
        ConfigurationCompletion::Indeterminate,
    ] {
        let mut machine = ConfigurationMachine::new(previous);
        let quiesce = effect(machine.begin(configured()));
        let apply = effect(machine.complete(quiesce.ticket, ConfigurationCompletion::Succeeded));
        let persist = effect(machine.complete(apply.ticket, ConfigurationCompletion::Succeeded));
        let restore =
            effect(machine.complete(persist.ticket, ConfigurationCompletion::Indeterminate));
        assert_eq!(
            restore.operation,
            ConfigurationOperation::RestoreHardware(previous)
        );
        let storage = effect(machine.complete(restore.ticket, hardware_result));
        assert_eq!(
            storage.operation,
            ConfigurationOperation::RestoreStorage(previous)
        );
        let result = machine.complete(storage.ticket, ConfigurationCompletion::Succeeded);
        assert_eq!(machine.durable(), DurableConfiguration::Confirmed(previous));
        if hardware_result == ConfigurationCompletion::Succeeded {
            let traffic = effect(result);
            assert_eq!(
                traffic.operation,
                ConfigurationOperation::RestoreTraffic(previous)
            );
            assert!(!machine.is_settled());
            assert_eq!(
                machine.complete(traffic.ticket, ConfigurationCompletion::Succeeded),
                ConfigurationOutcome::Failed(ConfigurationFailure::Persist)
            );
        } else {
            assert_eq!(
                result,
                ConfigurationOutcome::Failed(ConfigurationFailure::RestoreHardware)
            );
            assert_eq!(machine.hardware(), HardwareConfiguration::Unknown);
        }
        assert!(machine.is_settled());
    }
}
