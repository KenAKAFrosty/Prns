use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use prns_core::interfaces::lora::{
    AirtimePolicyError, RadioProfile, RadioProfileCompatibilityError, RadioProfileError,
};
use prns_core::interfaces::subghz::{ResolvedSubGMode, SubGConfigurationState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaApplyOutcome {
    Applied,
    Rejected(LoRaConfigurationRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaRadioConfigurationOperation {
    Initialize,
    ArmReceive,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousLoRaConfigurationRecovery {
    NotAttempted,
    Restored,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfigurationRejection {
    Profile(RadioProfileError),
    RadioCompatibility(RadioProfileCompatibilityError),
    AirtimePolicy(AirtimePolicyError),
    Radio {
        operation: LoRaRadioConfigurationOperation,
        previous: PreviousLoRaConfigurationRecovery,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LoRaApplyRequest {
    pub(super) id: u32,
    pub(super) command: LoRaConfigurationCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoRaConfigurationCommand {
    Apply(RadioProfile),
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoRaApplyResult {
    id: u32,
    outcome: LoRaApplyOutcome,
}

pub struct LoRaControl {
    requests: Signal<CriticalSectionRawMutex, LoRaApplyRequest>,
    results: Signal<CriticalSectionRawMutex, LoRaApplyResult>,
    request_gate: Mutex<CriticalSectionRawMutex, u32>,
}

impl LoRaControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requests: Signal::new(),
            results: Signal::new(),
            request_gate: Mutex::new(1),
        }
    }

    async fn request(&self, command: LoRaConfigurationCommand) -> LoRaApplyOutcome {
        let mut next_id = self.request_gate.lock().await;
        let id = *next_id;
        *next_id = id.wrapping_add(1);
        self.requests.signal(LoRaApplyRequest { id, command });
        loop {
            let result = self.results.wait().await;
            if result.id == id {
                return result.outcome;
            }
        }
    }

    pub fn apply(
        &self,
        profile: RadioProfile,
    ) -> impl core::future::Future<Output = LoRaApplyOutcome> + '_ {
        self.request(LoRaConfigurationCommand::Apply(profile))
    }

    pub fn clear(&self) -> impl core::future::Future<Output = LoRaApplyOutcome> + '_ {
        self.request(LoRaConfigurationCommand::Clear)
    }

    pub fn apply_configuration(
        &self,
        configuration: SubGConfigurationState,
    ) -> impl core::future::Future<Output = LoRaApplyOutcome> + '_ {
        let command = match configuration {
            SubGConfigurationState::Unconfigured => LoRaConfigurationCommand::Clear,
            SubGConfigurationState::Configured(configuration) => {
                let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                LoRaConfigurationCommand::Apply(profile)
            }
        };
        self.request(command)
    }

    pub(super) fn wait(&self) -> impl core::future::Future<Output = LoRaApplyRequest> + '_ {
        self.requests.wait()
    }

    pub(super) fn complete(&self, id: u32, outcome: LoRaApplyOutcome) {
        self.results.signal(LoRaApplyResult { id, outcome });
    }
}

impl Default for LoRaControl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll};
    use embassy_futures::join::{join, join3};
    use prns_core::interfaces::lora::TxPower;
    use prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
    use std::boxed::Box;
    use std::task::Waker;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn stale_results_cannot_settle_a_new_request() {
        let control = LoRaControl::new();
        control.results.signal(LoRaApplyResult {
            id: 0,
            outcome: LoRaApplyOutcome::Applied,
        });
        let requested = US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(12))
            .unwrap();
        let rejected = LoRaConfigurationRejection::Radio {
            operation: LoRaRadioConfigurationOperation::Initialize,
            previous: PreviousLoRaConfigurationRecovery::NotAttempted,
        };
        let (outcome, ()) = block_on(join(control.apply(requested), async {
            let request = control.wait().await;
            assert_eq!(request.command, LoRaConfigurationCommand::Apply(requested));
            control.complete(request.id, LoRaApplyOutcome::Rejected(rejected));
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Rejected(rejected));
    }

    #[test]
    fn concurrent_callers_each_receive_their_own_ordered_result() {
        let control = LoRaControl::new();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let rejected = LoRaConfigurationRejection::Radio {
            operation: LoRaRadioConfigurationOperation::ArmReceive,
            previous: PreviousLoRaConfigurationRecovery::Restored,
        };
        let (first_outcome, second_outcome, ()) =
            block_on(join3(control.apply(first), control.apply(second), async {
                let first_request = control.wait().await;
                assert_eq!(
                    first_request.command,
                    LoRaConfigurationCommand::Apply(first)
                );
                control.complete(first_request.id, LoRaApplyOutcome::Applied);
                let second_request = control.wait().await;
                assert_eq!(
                    second_request.command,
                    LoRaConfigurationCommand::Apply(second)
                );
                control.complete(second_request.id, LoRaApplyOutcome::Rejected(rejected));
            }));
        assert_eq!(first_outcome, LoRaApplyOutcome::Applied);
        assert_eq!(second_outcome, LoRaApplyOutcome::Rejected(rejected));
    }

    #[test]
    fn cancellation_before_intake_replaces_the_abandoned_request() {
        let control = LoRaControl::new();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let mut cancelled = Box::pin(control.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        drop(cancelled);

        let (outcome, ()) = block_on(join(control.apply(second), async {
            let current = control.wait().await;
            assert_eq!(current.command, LoRaConfigurationCommand::Apply(second));
            control.complete(current.id, LoRaApplyOutcome::Applied);
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Applied);
    }

    #[test]
    fn cancellation_after_intake_cannot_poison_the_next_request() {
        let control = LoRaControl::new();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let mut cancelled = Box::pin(control.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        let abandoned = block_on(control.wait());
        assert_eq!(abandoned.command, LoRaConfigurationCommand::Apply(first));
        drop(cancelled);

        let (outcome, ()) = block_on(join(control.apply(second), async {
            control.complete(abandoned.id, LoRaApplyOutcome::Applied);
            let current = control.wait().await;
            assert_eq!(current.command, LoRaConfigurationCommand::Apply(second));
            control.complete(current.id, LoRaApplyOutcome::Applied);
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Applied);
    }
}
