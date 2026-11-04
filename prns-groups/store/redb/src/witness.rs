use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A coalesced, process-local witness that a domain commit was attempted.
///
/// This is not a durable revision or proof of success. Share clones only among
/// writers on one exclusive domain owner lane; that owner alone drains the
/// witness after execution and publishes its observation outside transactions.
/// Callers and reply waiters never clear it. No callback or executor is stored.
#[derive(Clone, Default)]
pub struct ChangeWitness(Arc<AtomicBool>);

impl ChangeWitness {
    /// Mark immediately before committing. An error may still follow durable
    /// effects, so successful and uncertain commit outcomes both remain visible.
    pub fn mark_commit_attempt(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Drain all attempts since the previous owner publication into one hint.
    /// A write racing with the drain remains visible in this or the next drain.
    pub fn take_possible_change(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::ChangeWitness;

    #[test]
    fn clones_coalesce_without_claiming_a_committed_result() {
        let owner = ChangeWitness::default();
        let writer = owner.clone();
        assert!(!owner.take_possible_change());
        writer.mark_commit_attempt();
        writer.mark_commit_attempt();
        assert!(owner.take_possible_change());
        assert!(!owner.take_possible_change());
        writer.mark_commit_attempt();
        assert!(owner.take_possible_change());
    }
}
