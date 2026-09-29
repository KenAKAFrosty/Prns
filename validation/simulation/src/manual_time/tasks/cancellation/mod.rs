use super::{ManualTaskId, ManualTaskRunner, ManualTimeError, ReadyTasks};

#[derive(Debug, PartialEq, Eq)]
pub enum ManualTaskCancellation {
    Cancelled,
    /// The runner no longer owns this ID, or never admitted it.
    NotLive,
}

impl<T> Drop for ManualTaskRunner<'_, T> {
    fn drop(&mut self) {
        // Disable every wake before any actor destructor can wake a sibling.
        {
            let mut ready = ReadyTasks::lock(&self.ready);
            for &id in self.tasks.keys() {
                ready.retire(id);
            }
        }
        let _entered = self.driver.runtime.enter();
        self.tasks.clear();
    }
}

impl<T> ManualTaskRunner<'_, T> {
    /// Removes one actor without polling it or advancing time. IDs belong to
    /// their admitting runner; a cancelled ID is never reassigned by that runner.
    /// Cancellation discards the future, not an output, and frees its capacity.
    ///
    /// Rust destructors run in the driver's runtime context. This models host
    /// actor teardown, not physical power loss: no graceful shutdown is polled,
    /// but destructor side effects still occur and can wake surviving actors.
    /// Destructors must obey the same nonblocking/no-spawn rules as task polls.
    ///
    /// Clock validation precedes mutation. If validation fails after destruction,
    /// the actor is already gone and its effects cannot be rolled back; discard
    /// the runner just as for a post-poll validation failure.
    pub fn cancel(&mut self, id: ManualTaskId) -> Result<ManualTaskCancellation, ManualTimeError> {
        self.driver.validate()?;
        let Some(task) = self.tasks.remove(&id) else {
            return Ok(ManualTaskCancellation::NotLive);
        };
        // Retire before Drop can use a saved waker. Never hold the ready-index
        // lock across user destruction, which can also wake another live actor.
        ReadyTasks::lock(&self.ready).retire(id);
        {
            let _entered = self.driver.runtime.enter();
            drop(task);
        }
        self.driver.validate()?;
        Ok(ManualTaskCancellation::Cancelled)
    }
}

#[cfg(test)]
mod tests;
