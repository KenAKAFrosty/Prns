//! Disposable delivery scheduling hints over the durable authored-record outbox.
//! No ciphertext, receipt or delivery obligation is owned by this module.
use crate::{Client, Error};
use prns_groups_core::sync::SyncWake;
use prns_groups_service::{ClockReading, Endpoint, EnvelopeInfo, InventoryCursor};

#[derive(Clone, Copy, Default)]
pub(crate) struct PushState {
    after: u64,
    highest_attempted: u64,
    probe_after: Option<tokio::time::Instant>,
    consecutive: u8,
}

impl PushState {
    pub(crate) fn repair_due(self) -> bool {
        self.consecutive >= 4
    }

    pub(crate) fn repaired(&mut self) {
        self.consecutive = 0;
    }

    pub(crate) fn enabled(self, now: tokio::time::Instant) -> bool {
        self.probe_after.is_none_or(|after| now >= after)
    }

    pub(crate) fn unsupported(&mut self, now: tokio::time::Instant) {
        // A lossy read-only probe is not permanent evidence of incompatibility.
        // Avoid paying the negotiation timeout on every recovery poll, while
        // allowing upgraded/recovered peers to advertise support later.
        self.probe_after = Some(now + std::time::Duration::from_secs(300));
    }

    /// Prefer unattempted records, then round-robin retained obligations. The
    /// service rechecks membership, expiration and durable receipts on each read.
    pub(crate) async fn pending(
        self,
        client: &Client,
        peer: Endpoint,
        clock: ClockReading,
    ) -> Result<Option<EnvelopeInfo>, Error> {
        let fresh = client
            .pending_delivery(peer, InventoryCursor(self.highest_attempted), clock)
            .await?;
        if fresh.is_some_and(|info| self.is_new(info)) || self.after == self.highest_attempted {
            return Ok(fresh);
        }
        client
            .pending_delivery(peer, InventoryCursor(self.after), clock)
            .await
    }

    /// Recovery is useful only while an outbound obligation exists. Inbound
    /// repair traffic with an empty outbox must not create reciprocal polling.
    pub(crate) fn wake_evidence(
        self,
        pending: Option<EnvelopeInfo>,
        recovered: bool,
    ) -> Option<SyncWake> {
        pending.map(|info| {
            if self.is_new(info) {
                SyncWake::FreshWork
            } else if recovered {
                SyncWake::PeerRecovered
            } else {
                SyncWake::PendingWork
            }
        })
    }

    pub(crate) fn is_new(self, info: EnvelopeInfo) -> bool {
        info.position.0 > self.highest_attempted
    }

    pub(crate) fn attempted(&mut self, info: EnvelopeInfo) {
        self.after = info.position.0;
        self.highest_attempted = self.highest_attempted.max(info.position.0);
    }

    /// Fairness counts completed Submit exchanges, including remote rejection.
    /// A failed path/handshake did no push work and must not force an empty
    /// inventory round before the first delivery after the path recovers.
    pub(crate) fn exchanged(&mut self) {
        self.consecutive = self.consecutive.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_groups_service::{EnvelopeId, InventoryKind, RecordId};

    fn envelope(position: u64) -> EnvelopeInfo {
        EnvelopeInfo {
            position: InventoryCursor(position),
            id: EnvelopeId([1; 32]),
            kind: InventoryKind::Record,
            record: RecordId([2; 32]),
            bytes: 128,
        }
    }

    #[test]
    fn persistent_push_work_yields_to_inventory_repair_and_probe_cache_expires() {
        let mut state = PushState::default();
        for position in 1..=4 {
            assert!(!state.repair_due());
            state.attempted(envelope(position));
            state.exchanged();
        }
        assert!(state.repair_due());
        state.repaired();
        assert!(!state.repair_due());
        let now = tokio::time::Instant::now();
        state.unsupported(now);
        assert!(!state.enabled(now));
        assert!(!state.enabled(now + std::time::Duration::from_secs(299)));
        assert!(state.enabled(now + std::time::Duration::from_secs(300)));
    }

    #[test]
    fn failed_transport_preserves_retry_watermark_without_spending_repair_budget() {
        let mut state = PushState::default();
        for _ in 0..20 {
            state.attempted(envelope(1));
        }
        assert!(!state.is_new(envelope(1)));
        assert!(!state.repair_due());
        // An authenticated rejection is still a completed exchange: four of
        // these must yield to inventory repair just like successful delivery.
        for _ in 0..4 {
            state.exchanged();
        }
        assert!(state.repair_due());
    }

    #[test]
    fn authenticated_reciprocal_repair_without_outbox_cannot_accelerate() {
        let mut state = PushState::default();
        assert_eq!(state.wake_evidence(None, true), None);
        state.attempted(envelope(1));
        assert_eq!(state.wake_evidence(None, true), None);
        assert_eq!(
            state.wake_evidence(Some(envelope(1)), false),
            Some(SyncWake::PendingWork)
        );
        assert_eq!(
            state.wake_evidence(Some(envelope(1)), true),
            Some(SyncWake::PeerRecovered)
        );
        assert_eq!(
            state.wake_evidence(Some(envelope(2)), false),
            Some(SyncWake::FreshWork)
        );
    }

    #[test]
    fn retry_rotation_does_not_turn_old_work_into_fresh_work() {
        let mut state = PushState::default();
        assert!(state.is_new(envelope(1)));
        state.attempted(envelope(1));
        assert!(state.is_new(envelope(3)));
        state.attempted(envelope(3));
        state.attempted(envelope(1));
        assert_eq!(state.after, 1);
        assert!(!state.is_new(envelope(3)));
        assert!(state.is_new(envelope(4)));
    }
}
