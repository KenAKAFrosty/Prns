#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlAuthorizationRestoreOutcome {
    pub restored_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAuthorizationRestoreError {
    Unavailable,
    CapacityExhausted,
}
