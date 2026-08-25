//! Pure display-change tracking and refresh planning.

mod exact;
mod planner;

pub use exact::{
    ExactFrameState, ExactFrameTracker, ExactPresentationDecision, ExactPresentationError,
    ExactPresentationState, FrameChange, PresentationAttempt, PresentationBusy, UnknownReason,
};
pub use planner::{
    FeedbackError, ImmediateDisplayPolicy, MonotonicMillis, NonZeroDuration, PartialRefreshLimit,
    PlannerError, PolicyError, PresentationPolicy, PresentationSpacing, PresentationUrgency,
    RefreshAttempt, RefreshDecision, RefreshKind, RefreshPlanner, RetainedFullWaveformOnlyPolicy,
    RetainedPartialWaveformPolicy, RetryBackoff, ZeroDuration, ZeroPartialRefreshLimit,
};
