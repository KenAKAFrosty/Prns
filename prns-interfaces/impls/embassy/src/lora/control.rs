use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use prns_core::interfaces::lora::{LoRaConfigurationState, LoRaProfile as RadioProfile};
use prns_runtime::manifold::driver::InterfacePublicationOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaApplyOutcome {
    Applied,
    Rejected,
    IdentityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LoRaApplyRequest {
    pub(super) id: u64,
    pub(super) command: LoRaConfigurationCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoRaConfigurationCommand {
    Apply(RadioProfile),
    Clear,
    Quiesce,
    Stage(LoRaConfigurationState),
    Publish,
    Resume,
    Hold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoRaApplyResult {
    id: u64,
    outcome: LoRaApplyOutcome,
}

pub struct LoRaControl {
    requests: Signal<CriticalSectionRawMutex, LoRaApplyRequest>,
    results: Signal<CriticalSectionRawMutex, LoRaApplyResult>,
    publication: Signal<CriticalSectionRawMutex, InterfacePublicationOutcome>,
}

pub struct LoRaController<'a> {
    control: &'a LoRaControl,
    next_id: u64,
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
            publication: Signal::new(),
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
        let Some(next_id) = id.checked_add(1) else {
            return LoRaApplyOutcome::IdentityExhausted;
        };
        self.next_id = next_id;
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

    pub async fn quiesce(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Quiesce).await
    }
    pub async fn stage_configuration(
        &mut self,
        configuration: LoRaConfigurationState,
    ) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Stage(configuration))
            .await
    }
    pub async fn publish_configuration(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Publish).await
    }
    pub async fn resume_configuration(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Resume).await
    }
    pub async fn hold_configuration(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Hold).await
    }

    pub async fn apply(&mut self, profile: impl Into<RadioProfile>) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Apply(profile.into()))
            .await
    }

    pub async fn clear(&mut self) -> LoRaApplyOutcome {
        self.request(LoRaConfigurationCommand::Clear).await
    }

    pub async fn apply_configuration<C: Into<LoRaConfigurationState>>(
        &mut self,
        configuration: C,
    ) -> LoRaApplyOutcome {
        let command = match configuration.into() {
            LoRaConfigurationState::Unconfigured => LoRaConfigurationCommand::Clear,
            LoRaConfigurationState::Configured(configuration) => {
                let profile = configuration.profile();
                LoRaConfigurationCommand::Apply(profile)
            }
        };
        self.request(command).await
    }
}

impl<'a> LoRaControlTarget<'a> {
    pub(super) fn has_pending(&self) -> bool {
        self.control.requests.signaled()
    }

    pub(super) fn publication(
        &self,
    ) -> &'a Signal<CriticalSectionRawMutex, InterfacePublicationOutcome> {
        &self.control.publication
    }

    pub(super) fn wait(&self) -> impl core::future::Future<Output = LoRaApplyRequest> + '_ {
        self.control.requests.wait()
    }

    pub(super) fn complete(&self, id: u64, outcome: LoRaApplyOutcome) {
        self.control.results.signal(LoRaApplyResult { id, outcome });
    }
}

impl Default for LoRaControl {
    fn default() -> Self {
        Self::new()
    }
}

impl prns_core::interfaces::lora::configuration::ConfigurationRadio for LoRaController<'_> {
    async fn perform(
        &mut self,
        operation: prns_core::interfaces::lora::configuration::RadioConfigurationOperation,
    ) -> prns_core::interfaces::lora::configuration::ConfigurationCompletion {
        use prns_core::interfaces::lora::configuration::{
            ConfigurationCompletion, RadioConfigurationOperation,
        };
        let command = match operation {
            RadioConfigurationOperation::Quiesce => LoRaConfigurationCommand::Quiesce,
            RadioConfigurationOperation::Stage(configuration) => {
                LoRaConfigurationCommand::Stage(configuration)
            }
            RadioConfigurationOperation::Publish => LoRaConfigurationCommand::Publish,
            RadioConfigurationOperation::Resume => LoRaConfigurationCommand::Resume,
            RadioConfigurationOperation::Hold => LoRaConfigurationCommand::Hold,
        };
        match self.request(command).await {
            LoRaApplyOutcome::Applied => ConfigurationCompletion::Succeeded,
            LoRaApplyOutcome::Rejected | LoRaApplyOutcome::IdentityExhausted => {
                ConfigurationCompletion::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll};
    use embassy_futures::join::join;
    use prns_core::interfaces::lora::TxPower;
    const US915_AUTO_LORA_PROFILE: RadioProfile =
        RadioProfile::SubG(prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE);
    use std::boxed::Box;
    use std::task::Waker;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "control request failed to settle"
            );
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
        assert!(!target.has_pending());
        let mut cancelled = Box::pin(controller.apply(first));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        assert!(target.has_pending());
        let abandoned = block_on(target.wait());
        assert!(!target.has_pending());
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
    #[test]
    fn exhausted_identity_does_not_submit_or_wrap_a_request() {
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        controller.next_id = u64::MAX;
        assert_eq!(
            block_on(controller.quiesce()),
            LoRaApplyOutcome::IdentityExhausted
        );
        assert_eq!(controller.next_id, u64::MAX);
        assert!(!target.has_pending());
    }
    #[test]
    fn configuration_radio_adapter_preserves_every_operation_and_completion() {
        use prns_core::interfaces::lora::configuration::{
            ConfigurationCompletion, ConfigurationRadio, RadioConfigurationOperation as Operation,
        };
        for (operation, command) in [
            (Operation::Quiesce, LoRaConfigurationCommand::Quiesce),
            (
                Operation::Stage(LoRaConfigurationState::Unconfigured),
                LoRaConfigurationCommand::Stage(LoRaConfigurationState::Unconfigured),
            ),
            (Operation::Publish, LoRaConfigurationCommand::Publish),
            (Operation::Resume, LoRaConfigurationCommand::Resume),
            (Operation::Hold, LoRaConfigurationCommand::Hold),
        ] {
            for (outcome, expected) in [
                (
                    LoRaApplyOutcome::Applied,
                    ConfigurationCompletion::Succeeded,
                ),
                (LoRaApplyOutcome::Rejected, ConfigurationCompletion::Failed),
            ] {
                let mut control = LoRaControl::default();
                let (mut controller, target) = control.split();
                let (result, ()) = block_on(join(controller.perform(operation), async {
                    let request = target.wait().await;
                    assert_eq!(request.command, command);
                    target.complete(request.id, outcome);
                }));
                assert_eq!(result, expected);
            }
        }
        let mut control = LoRaControl::default();
        let (mut controller, target) = control.split();
        let (result, ()) = block_on(join(
            controller.apply_configuration(LoRaConfigurationState::Unconfigured),
            async {
                let request = target.wait().await;
                assert_eq!(request.command, LoRaConfigurationCommand::Clear);
                target.complete(request.id, LoRaApplyOutcome::Applied);
            },
        ));
        assert_eq!(result, LoRaApplyOutcome::Applied);
        controller.next_id = u64::MAX;
        assert_eq!(
            block_on(controller.perform(Operation::Hold)),
            ConfigurationCompletion::Failed
        );
    }
}
