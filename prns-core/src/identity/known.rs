use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownDestinationRetentionState {
    NeverUsed,
    UsedAt(InstantMillis),
    Retained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkDestinationUsedOutcome {
    Recorded,
    Refreshed,
    Retained,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainDestinationOutcome {
    Retained,
    AlreadyRetained,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDestinationOutcome {
    Released,
    UseRecorded,
    UseRefreshed,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainIdentityOutcome {
    pub newly_retained_destination_count: u32,
    pub already_retained_destination_count: u32,
}
