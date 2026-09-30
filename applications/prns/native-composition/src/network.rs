//! Bounded application activity, separate from authoritative routes and LXMF discovery.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use prns_host_native::embedding::NativeAnnounceObserver;
use prns_host_native::AuthenticatedAnnounce;

use crate::contract::{
    LocalAnnounceActivity, LocalNetworkRouteSnapshot, LocalNetworkSnapshot, LocalNetworkState,
};

const ACTIVITY_CAPACITY: usize = 200;

pub(crate) struct NetworkActivityStore {
    pub(crate) generation_id: u64,
    admitted: AtomicU64,
    inner: Mutex<ActivityRing>,
}

struct ActivityRing {
    rows: VecDeque<Observation>,
    revision: u64,
    cleared_through: u64,
    evictions: u64,
}

/// Only fixed-size authenticated metadata survives the callback. No app payload,
/// public key material, messaging name, or contact association is retained here.
#[derive(Clone, Copy)]
struct Observation {
    record_id: u64,
    destination: [u8; 16],
    announced_identity: [u8; 16],
    source_interface: [u8; 8],
    hops: u8,
    arrived_at_millis: u64,
    is_path_response: bool,
}

pub(crate) struct ActivityProjection {
    pub(crate) revision: u64,
    pub(crate) evictions: u64,
    pub(crate) rows: Vec<LocalAnnounceActivity>,
}

impl NetworkActivityStore {
    pub(crate) fn new(generation_id: u64) -> Self {
        Self {
            generation_id,
            admitted: AtomicU64::new(0),
            inner: Mutex::new(ActivityRing {
                rows: VecDeque::with_capacity(ACTIVITY_CAPACITY),
                revision: 0,
                cleared_through: 0,
                evictions: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ActivityRing> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn compose_observer(
        self: &Arc<Self>,
        mut existing: impl for<'a> FnMut(AuthenticatedAnnounce<'a>) + Send + 'static,
    ) -> NativeAnnounceObserver {
        let activity = Arc::clone(self);
        Box::new(move |observation| {
            let admitted = activity.admit(observation);
            // Preserve the existing authenticated LXMF observer exactly once,
            // outside the ring lock, even if no activity ID remains available.
            existing(observation);
            if let Some(admitted) = admitted {
                activity.record(admitted);
            }
        })
    }

    fn admit(&self, observation: AuthenticatedAnnounce<'_>) -> Option<Observation> {
        let previous = self
            .admitted
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
                last.checked_add(1)
            })
            .ok()?;
        Some(Observation {
            record_id: previous + 1,
            destination: observation.destination.into_bytes(),
            announced_identity: observation.announced_identity.into_bytes(),
            source_interface: observation.source_interface.into_bytes(),
            hops: observation.hops,
            arrived_at_millis: observation.arrived_at_millis,
            is_path_response: observation.is_path_response,
        })
    }

    fn record(&self, observation: Observation) {
        let mut ring = self.lock();
        if observation.record_id <= ring.cleared_through {
            return;
        }
        ring.revision = ring.revision.saturating_add(1);
        if ring.rows.len() == ACTIVITY_CAPACITY {
            ring.evictions = ring.evictions.saturating_add(1);
            if ring
                .rows
                .front()
                .is_some_and(|oldest| oldest.record_id > observation.record_id)
            {
                // This delayed row itself falls outside the latest-200 window.
                return;
            }
            ring.rows.pop_front();
        }
        let index = ring
            .rows
            .iter()
            .position(|row| row.record_id > observation.record_id)
            .unwrap_or(ring.rows.len());
        ring.rows.insert(index, observation);
    }

    pub(crate) fn clear(&self) -> u64 {
        let mut ring = self.lock();
        // Admission sequence, not time, defines clear. A delayed older callback
        // cannot return, but a later callback in the same clock tick is retained.
        ring.cleared_through = self.admitted.load(Ordering::SeqCst);
        ring.rows.clear();
        ring.evictions = 0;
        ring.revision = ring.revision.saturating_add(1);
        ring.revision
    }

    pub(crate) fn project(&self, now_millis: u64) -> ActivityProjection {
        // Release the ring lock before allocating foreign fields or taking the
        // aggregate snapshot lock. Neither lock is held across an await.
        let (revision, evictions, rows) = {
            let ring = self.lock();
            (ring.revision, ring.evictions, ring.rows.clone())
        };
        ActivityProjection {
            revision,
            evictions,
            rows: rows
                .into_iter()
                .rev()
                .map(|row| LocalAnnounceActivity {
                    record_id: row.record_id,
                    destination: row.destination,
                    announced_identity: row.announced_identity,
                    source_interface: row.source_interface.to_vec(),
                    hops: row.hops,
                    age_millis: now_millis.saturating_sub(row.arrived_at_millis),
                    is_path_response: row.is_path_response,
                })
                .collect(),
        }
    }

    pub(crate) fn project_network(
        &self,
        host: &prns_host::HostSnapshot,
        now: u64,
    ) -> LocalNetworkSnapshot {
        let activity = self.project(now);
        LocalNetworkSnapshot {
            state: LocalNetworkState::Ready,
            routes: project_routes(&host.routes, now),
            announces: activity.rows,
            activity_revision: activity.revision,
            dropped_announce_count: activity.evictions,
        }
    }
}

fn project_routes(routes: &[prns_host::RouteSnapshot], now: u64) -> Vec<LocalNetworkRouteSnapshot> {
    let mut routes: Vec<_> = routes
        .iter()
        .map(|route| LocalNetworkRouteSnapshot {
            destination: route.destination.into_bytes(),
            via_identity: route.via_identity.map(prns_host::IdentityHash::into_bytes),
            interface_id: route.interface_id.as_bytes().to_vec(),
            hops: route.hops,
            learned_age_millis: now.saturating_sub(route.learned_at_millis),
            last_activity_age_millis: now.saturating_sub(route.last_route_activity_at_millis),
            expires_in_millis: route.expires_at_millis.saturating_sub(now),
            expired: route.expires_at_millis <= now,
        })
        .collect();
    routes.sort_by(|left, right| {
        left.destination
            .cmp(&right.destination)
            .then_with(|| left.interface_id.cmp(&right.interface_id))
    });
    routes
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;
