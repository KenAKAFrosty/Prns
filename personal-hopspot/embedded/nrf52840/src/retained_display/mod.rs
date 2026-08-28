use personal_hopspot_core::display::{
    DisplayCoordinator, EinkPolicy, MonotonicMillis, PresentationDecision, PresentationError,
    PresentationOutcome, PresentationPolicy, PresentationUrgency, RefreshKind, UserBlankingPolicy,
};
use personal_hopspot_core::face_64x128::{Frame, RenderInput};
use personal_hopspot_core::UserBlanking;

pub(crate) enum BoardDisplay<D> {
    Initialized(D),
    InitializationFailed(D),
}

impl<D> BoardDisplay<D> {
    pub(crate) const fn initialized(device: D) -> Self {
        Self::Initialized(device)
    }

    pub(crate) const fn initialization_failed(device: D) -> Self {
        Self::InitializationFailed(device)
    }

    pub(crate) fn into_runtime(self, policy: EinkPolicy) -> RetainedDisplayRuntime<D> {
        let (device, controller) = match self {
            Self::Initialized(device) => (device, ControllerState::Ready),
            Self::InitializationFailed(device) => (device, ControllerState::RecoveryRequired),
        };
        RetainedDisplayRuntime {
            device,
            coordinator: DisplayCoordinator::available(
                PresentationPolicy::RetainedEink(policy),
                UserBlankingPolicy::Unavailable,
            ),
            controller,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetainedRefresh {
    Full,
    Partial,
}

pub(crate) trait RetainedDisplayDevice {
    type Error;

    async fn present(&mut self, frame: &Frame, refresh: RetainedRefresh)
        -> Result<(), Self::Error>;
    async fn recover(&mut self) -> Result<(), Self::Error>;
    async fn deep_sleep(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetainedPresentation {
    Presented,
    Unchanged,
    DeferredUntil(MonotonicMillis),
    Sleeping,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum RetainedDisplayError<E> {
    Coordinator(PresentationError),
    ImmediateRefresh,
    Device(E),
}

impl<E> From<PresentationError> for RetainedDisplayError<E> {
    fn from(error: PresentationError) -> Self {
        Self::Coordinator(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControllerState {
    Ready,
    RecoveryRequired,
    Sleeping,
}

pub(crate) struct RetainedDisplayRuntime<D> {
    device: D,
    coordinator: DisplayCoordinator,
    controller: ControllerState,
}

impl<D: RetainedDisplayDevice> RetainedDisplayRuntime<D> {
    pub(crate) const fn user_blanking(&self) -> UserBlanking {
        self.coordinator.user_blanking()
    }

    pub(crate) async fn render_and_present(
        &mut self,
        input: RenderInput<'_, '_>,
        planned_at: MonotonicMillis,
        urgency: PresentationUrgency,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<RetainedPresentation, RetainedDisplayError<D::Error>> {
        match self.controller {
            ControllerState::Ready => {}
            ControllerState::RecoveryRequired => self.recover().await?,
            ControllerState::Sleeping => return Ok(RetainedPresentation::Sleeping),
        }

        self.coordinator.render(input)?;
        match self.coordinator.plan_presentation(planned_at, urgency)? {
            PresentationDecision::Unchanged => Ok(RetainedPresentation::Unchanged),
            PresentationDecision::DeferredUntil(deadline) => {
                Ok(RetainedPresentation::DeferredUntil(deadline))
            }
            PresentationDecision::Present(attempt) => {
                let refresh = match attempt.refresh_kind() {
                    RefreshKind::Full => RetainedRefresh::Full,
                    RefreshKind::Partial => RetainedRefresh::Partial,
                    RefreshKind::Immediate => {
                        self.coordinator.invalidate_presentation()?;
                        return Err(RetainedDisplayError::ImmediateRefresh);
                    }
                };
                let outcome = self
                    .device
                    .present(self.coordinator.candidate_frame(&attempt)?, refresh)
                    .await;
                let completed_at = completed_at();
                self.coordinator.complete_presentation(
                    attempt,
                    completed_at,
                    if outcome.is_ok() {
                        PresentationOutcome::Succeeded
                    } else {
                        PresentationOutcome::Failed
                    },
                )?;
                match outcome {
                    Ok(()) => Ok(RetainedPresentation::Presented),
                    Err(error) => {
                        self.controller = ControllerState::RecoveryRequired;
                        Err(RetainedDisplayError::Device(error))
                    }
                }
            }
        }
    }

    pub(crate) async fn deep_sleep(&mut self) -> Result<(), RetainedDisplayError<D::Error>> {
        match self.controller {
            ControllerState::Sleeping => return Ok(()),
            ControllerState::RecoveryRequired => self.recover().await?,
            ControllerState::Ready => {}
        }
        let result = self.device.deep_sleep().await;
        self.coordinator.invalidate_presentation()?;
        match result {
            Ok(()) => {
                self.controller = ControllerState::Sleeping;
                Ok(())
            }
            Err(error) => {
                self.controller = ControllerState::RecoveryRequired;
                Err(RetainedDisplayError::Device(error))
            }
        }
    }

    pub(crate) async fn wake(&mut self) -> Result<(), RetainedDisplayError<D::Error>> {
        match self.controller {
            ControllerState::Ready => Ok(()),
            ControllerState::RecoveryRequired | ControllerState::Sleeping => self.recover().await,
        }
    }

    async fn recover(&mut self) -> Result<(), RetainedDisplayError<D::Error>> {
        self.coordinator.invalidate_presentation()?;
        match self.device.recover().await {
            Ok(()) => {
                self.controller = ControllerState::Ready;
                Ok(())
            }
            Err(error) => {
                self.controller = ControllerState::RecoveryRequired;
                Err(RetainedDisplayError::Device(error))
            }
        }
    }

    #[cfg(test)]
    const fn device(&self) -> &D {
        &self.device
    }
}

#[cfg(test)]
mod tests;
