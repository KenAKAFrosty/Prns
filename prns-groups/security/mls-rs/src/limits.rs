/// Local admission and persistence limits. Upstream MLS may allocate and derive
/// keys before an expanded snapshot is checked against its storage limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_members: usize,
    pub max_groups: usize,
    pub max_wire_bytes: usize,
    pub max_plaintext_bytes: usize,
    pub max_snapshot_bytes: usize,
    pub max_key_packages: usize,
    pub max_retained_epochs: usize,
    pub max_pending_proposals: usize,
    pub max_pending_outputs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Configuration,
    Members,
    Groups,
    WireBytes,
    PlaintextBytes,
    SnapshotBytes,
    KeyPackages,
    Proposals,
    PendingOutputs,
    Allocation,
}

impl Limits {
    pub fn validate(&self) -> Result<(), Limit> {
        if self.max_members == 0
            || self.max_members >= u32::MAX as usize
            || self.max_groups == 0
            || self.max_key_packages == 0
            || self.max_wire_bytes == 0
            || self.max_plaintext_bytes == 0
            || self.max_snapshot_bytes == 0
            || self.max_pending_outputs == 0
            || self.max_pending_proposals == 0
            || self.max_plaintext_bytes > self.max_wire_bytes
            || self.fields().iter().any(|n| *n > u32::MAX as usize)
        {
            return Err(Limit::Configuration);
        }
        Ok(())
    }

    pub(crate) fn fields(&self) -> [usize; 9] {
        [
            self.max_members,
            self.max_groups,
            self.max_wire_bytes,
            self.max_plaintext_bytes,
            self.max_snapshot_bytes,
            self.max_key_packages,
            self.max_retained_epochs,
            self.max_pending_proposals,
            self.max_pending_outputs,
        ]
    }
}
