use alloc::string::String;

use crate::{LifecyclePhase, PrnsLimits, StopReason};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostFailure {
    EventBackpressureExceeded {
        limits: PrnsLimits,
        rejected_event_bytes: usize,
    },
    BackendFailed {
        component: String,
        detail: String,
    },
    ContractViolated {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    Starting,
    Running,
    Stopping,
    Stopped(StopReason),
    Failed(HostFailure),
}

impl LifecycleState {
    #[must_use]
    pub const fn phase(&self) -> LifecyclePhase {
        match self {
            Self::Starting => LifecyclePhase::Starting,
            Self::Running => LifecyclePhase::Running,
            Self::Stopping => LifecyclePhase::Stopping,
            Self::Stopped(_) => LifecyclePhase::Stopped,
            Self::Failed(_) => LifecyclePhase::Failed,
        }
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped(_) | Self::Failed(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    pub revision: u64,
    pub state: LifecycleState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleTransitionError {
    pub from: LifecyclePhase,
    pub to: LifecyclePhase,
}

pub(crate) fn transition_allowed(from: LifecyclePhase, to: LifecyclePhase) -> bool {
    matches!(
        (from, to),
        (
            LifecyclePhase::Starting,
            LifecyclePhase::Running | LifecyclePhase::Stopping | LifecyclePhase::Failed
        ) | (
            LifecyclePhase::Running,
            LifecyclePhase::Stopping | LifecyclePhase::Failed
        ) | (
            LifecyclePhase::Stopping,
            LifecyclePhase::Stopped | LifecyclePhase::Failed
        )
    )
}
