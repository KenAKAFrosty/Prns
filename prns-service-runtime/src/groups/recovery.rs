use super::*;

impl<const I: usize> Local<I> {
    /// Inspect at most one original physical attempt on the existing owner lane,
    /// including while it is closing. This cannot admit replacement work.
    pub fn poll_recovery(
        &mut self,
        context: &mut Context<'_>,
        authority: &mut dyn owned::RecoveryAdmission,
    ) {
        let count = self.entries.len();
        if count == 0 {
            return;
        }
        let Some(index) = (0..count)
            .map(|offset| (self.next_recovery + offset) % count)
            .find(|&index| {
                self.entries[index].carrier.is_some()
                    || self.entries[index].attachment.pending_carrier().is_some()
            })
        else {
            return;
        };
        self.next_recovery = (index + 1) % count;
        let entry = &mut self.entries[index];
        let result = self.stores.with_existing(entry.identity, |database| {
            // Neither half may be silently discarded. In particular a missing
            // continuation is never evidence that an original permit settled.
            if entry.carrier.is_none() || entry.attachment.pending_carrier().is_none() {
                return Err(SchemaError::Group(groups::Error::Configuration));
            }
            entry
                .attachment
                .recover_carrier(
                    database,
                    &mut admission::RecoveryGate {
                        identity: entry.identity,
                        admission: authority,
                        pending: &mut entry.carrier,
                    },
                )
                .map_err(SchemaError::Group)
        });
        match result {
            Ok(true) => {
                entry.dirty = true;
                entry.failure = None;
                // Observe changed durable facts and give another pending
                // identity a bounded progress turn before lifecycle release.
                context.waker().wake_by_ref();
            }
            Ok(false) => (),
            Err(error) => {
                // Preserve the original lease/key and expose the failed proof.
                // Retry on the next owner wake rather than spinning on I/O.
                entry.failure = Some(Arc::new(error));
            }
        }
    }
}
