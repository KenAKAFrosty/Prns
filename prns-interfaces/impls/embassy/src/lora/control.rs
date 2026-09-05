use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use portable_atomic::{AtomicU32, Ordering};
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
    requests: Channel<CriticalSectionRawMutex, LoRaApplyRequest, 1>,
    results: Channel<CriticalSectionRawMutex, LoRaApplyResult, 1>,
    request_gate: Mutex<CriticalSectionRawMutex, ()>,
    next_id: AtomicU32,
}

impl LoRaControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requests: Channel::new(),
            results: Channel::new(),
            request_gate: Mutex::new(()),
            next_id: AtomicU32::new(1),
        }
    }

    async fn request(&self, command: LoRaConfigurationCommand) -> LoRaApplyOutcome {
        let _request = self.request_gate.lock().await;
        while self.results.try_receive().is_ok() {}
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.requests.send(LoRaApplyRequest { id, command }).await;
        loop {
            let result = self.results.receive().await;
            if result.id == id {
                return result.outcome;
            }
        }
    }

    pub async fn apply(&self, profile: RadioProfile) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Apply(profile)).await
    }

    pub async fn clear(&self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Clear).await
    }

    pub async fn apply_configuration(
        &self,
        configuration: SubGConfigurationState,
    ) -> LoRaApplyOutcome {
        match configuration {
            SubGConfigurationState::Unconfigured => self.clear().await,
            SubGConfigurationState::Configured(configuration) => {
                let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                self.apply(profile).await
            }
        }
    }

    pub(super) async fn wait(&self) -> LoRaApplyRequest {
        self.requests.receive().await
    }

    pub(super) async fn complete(&self, id: u32, outcome: LoRaApplyOutcome) {
        self.results.send(LoRaApplyResult { id, outcome }).await;
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
        block_on(control.results.send(LoRaApplyResult {
            id: 0,
            outcome: LoRaApplyOutcome::Applied,
        }));
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
            control
                .complete(request.id, LoRaApplyOutcome::Rejected(rejected))
                .await;
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
                control
                    .complete(first_request.id, LoRaApplyOutcome::Applied)
                    .await;
                let second_request = control.wait().await;
                assert_eq!(
                    second_request.command,
                    LoRaConfigurationCommand::Apply(second)
                );
                control
                    .complete(second_request.id, LoRaApplyOutcome::Rejected(rejected))
                    .await;
            }));
        assert_eq!(first_outcome, LoRaApplyOutcome::Applied);
        assert_eq!(second_outcome, LoRaApplyOutcome::Rejected(rejected));
    }

    #[test]
    fn cancellation_after_submission_cannot_poison_the_next_request() {
        let control = LoRaControl::new();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let mut cancelled = Box::pin(control.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        drop(cancelled);

        let (outcome, ()) = block_on(join(control.apply(second), async {
            let abandoned = control.wait().await;
            assert_eq!(abandoned.command, LoRaConfigurationCommand::Apply(first));
            control
                .complete(abandoned.id, LoRaApplyOutcome::Applied)
                .await;
            let current = control.wait().await;
            assert_eq!(current.command, LoRaConfigurationCommand::Apply(second));
            control
                .complete(current.id, LoRaApplyOutcome::Applied)
                .await;
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Applied);
    }
}
