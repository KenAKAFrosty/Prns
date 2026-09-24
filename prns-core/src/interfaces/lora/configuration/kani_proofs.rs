use super::*;
use crate::interfaces::lora::LoRaConfiguration;
use crate::interfaces::subghz::regions::us915::Us915;

#[kani::proof]
#[kani::unwind(8)]
fn transaction_success_requires_hardware_and_durable_agreement() {
    let requested = LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()));
    let mut machine = ConfigurationMachine::new(LoRaConfigurationState::Unconfigured);
    assert!(machine.is_settled());
    let mut outcome = machine.begin(requested);
    for _ in 0..7 {
        match outcome {
            ConfigurationOutcome::Effect(effect) => {
                let choice: u8 = kani::any();
                kani::assume(choice < 3);
                let completion = match choice {
                    0 => ConfigurationCompletion::Succeeded,
                    1 => ConfigurationCompletion::Failed,
                    _ => ConfigurationCompletion::Indeterminate,
                };
                outcome = machine.complete(effect.ticket, completion);
                assert_eq!(
                    machine.complete(effect.ticket, completion),
                    ConfigurationOutcome::Stale
                );
            }
            ConfigurationOutcome::Saved => {
                assert_eq!(
                    machine.hardware(),
                    HardwareConfiguration::Confirmed(requested)
                );
                assert_eq!(
                    machine.durable(),
                    DurableConfiguration::Confirmed(requested)
                );
                assert!(machine.is_settled());
            }
            ConfigurationOutcome::Failed(_) => assert!(machine.is_settled()),
            ConfigurationOutcome::Refused { .. } | ConfigurationOutcome::Stale => assert!(false),
        }
    }
    assert!(machine.is_settled());
}

#[kani::proof]
#[kani::unwind(6)]
fn completed_configuration_tickets_cannot_settle_a_later_transaction() {
    let previous = LoRaConfigurationState::Unconfigured;
    let requested = LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()));
    let mut machine = ConfigurationMachine::new(previous);
    machine.next_transaction = kani::any();
    kani::assume(machine.next_transaction < u64::MAX - 1);
    let ConfigurationOutcome::Effect(first) = machine.begin(requested) else {
        unreachable!()
    };
    let mut outcome = ConfigurationOutcome::Effect(first);
    for _ in 0..4 {
        let ConfigurationOutcome::Effect(pending) = outcome else {
            unreachable!()
        };
        outcome = machine.complete(pending.ticket, ConfigurationCompletion::Succeeded);
    }
    assert_eq!(outcome, ConfigurationOutcome::Saved);
    let ConfigurationOutcome::Effect(second) = machine.begin(previous) else {
        unreachable!()
    };
    assert_ne!(first.ticket, second.ticket);
    assert_eq!(
        machine.complete(first.ticket, ConfigurationCompletion::Succeeded),
        ConfigurationOutcome::Stale
    );
    assert_eq!(
        machine.hardware(),
        HardwareConfiguration::Confirmed(requested)
    );
    assert_eq!(
        machine.durable(),
        DurableConfiguration::Confirmed(requested)
    );
    assert!(!machine.is_settled());
}
