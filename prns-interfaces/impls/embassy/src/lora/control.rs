use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use prns_core::interfaces::lora::RadioProfile;
use prns_core::interfaces::subghz::{ResolvedSubGMode, SubGConfigurationState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaApplyOutcome {
    Applied,
    Rejected,
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
}

pub struct LoRaController<'a> {
    control: &'a LoRaControl,
    next_id: u32,
}

pub struct LoRaControlTarget<'a> {
    control: &'a LoRaControl,
}

impl LoRaControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requests: Signal::new(),
            results: Signal::new(),
        }
    }

    pub fn split(&mut self) -> (LoRaController<'_>, LoRaControlTarget<'_>) {
        (
            LoRaController {
                control: self,
                next_id: 1,
            },
            LoRaControlTarget { control: self },
        )
    }
}

impl<'a> LoRaController<'a> {
    async fn request(&mut self, command: LoRaConfigurationCommand) -> LoRaApplyOutcome {
        let id = self.next_id;
        self.next_id = id.wrapping_add(1);
        self.control
            .requests
            .signal(LoRaApplyRequest { id, command });
        loop {
            let result = self.control.results.wait().await;
            if result.id == id {
                return result.outcome;
            }
        }
    }

    pub async fn apply(&mut self, profile: RadioProfile) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Apply(profile)).await
    }

    pub async fn clear(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Clear).await
    }

    pub async fn apply_configuration(
        &mut self,
        configuration: SubGConfigurationState,
    ) -> LoRaApplyOutcome {
        let command = match configuration {
            SubGConfigurationState::Unconfigured => LoRaConfigurationCommand::Clear,
            SubGConfigurationState::Configured(configuration) => {
                let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                LoRaConfigurationCommand::Apply(profile)
            }
        };
        self.request(command).await
    }
}

impl LoRaControlTarget<'_> {
    pub(super) fn wait(&self) -> impl core::future::Future<Output = LoRaApplyRequest> + '_ {
        self.control.requests.wait()
    }

    pub(super) fn complete(&self, id: u32, outcome: LoRaApplyOutcome) {
        self.control.results.signal(LoRaApplyResult { id, outcome });
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
    use embassy_futures::join::join;
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
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        target.complete(0, LoRaApplyOutcome::Applied);
        let requested = US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(12))
            .unwrap();
        let (outcome, ()) = block_on(join(controller.apply(requested), async {
            let request = target.wait().await;
            assert_eq!(request.command, LoRaConfigurationCommand::Apply(requested));
            target.complete(request.id, LoRaApplyOutcome::Rejected);
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Rejected);
    }

    #[test]
    fn sequential_requests_each_receive_their_own_ordered_result() {
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let ((first_outcome, second_outcome), ()) = block_on(join(
            async {
                let first_outcome = controller.apply(first).await;
                let second_outcome = controller.apply(second).await;
                (first_outcome, second_outcome)
            },
            async {
                let first_request = target.wait().await;
                assert_eq!(
                    first_request.command,
                    LoRaConfigurationCommand::Apply(first)
                );
                target.complete(first_request.id, LoRaApplyOutcome::Applied);
                let second_request = target.wait().await;
                assert_eq!(
                    second_request.command,
                    LoRaConfigurationCommand::Apply(second)
                );
                target.complete(second_request.id, LoRaApplyOutcome::Rejected);
            },
        ));
        assert_eq!(first_outcome, LoRaApplyOutcome::Applied);
        assert_eq!(second_outcome, LoRaApplyOutcome::Rejected);
    }

    #[test]
    fn cancellation_before_intake_replaces_the_abandoned_request() {
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let mut cancelled = Box::pin(controller.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        drop(cancelled);

        let (outcome, ()) = block_on(join(controller.apply(second), async {
            let current = target.wait().await;
            assert_eq!(current.command, LoRaConfigurationCommand::Apply(second));
            target.complete(current.id, LoRaApplyOutcome::Applied);
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Applied);
    }

    #[test]
    fn cancellation_after_intake_cannot_poison_the_next_request() {
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let first = US915_AUTO_LORA_PROFILE;
        let second = first.with_tx_power(TxPower::new(12)).unwrap();
        let mut cancelled = Box::pin(controller.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        let abandoned = block_on(target.wait());
        assert_eq!(abandoned.command, LoRaConfigurationCommand::Apply(first));
        drop(cancelled);

        let (outcome, ()) = block_on(join(controller.apply(second), async {
            target.complete(abandoned.id, LoRaApplyOutcome::Applied);
            let current = target.wait().await;
            assert_eq!(current.command, LoRaConfigurationCommand::Apply(second));
            target.complete(current.id, LoRaApplyOutcome::Applied);
        }));
        assert_eq!(outcome, LoRaApplyOutcome::Applied);
    }
}
