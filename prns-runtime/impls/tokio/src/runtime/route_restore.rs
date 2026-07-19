use core::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use crate::engine::{
    EngineState, InstantMillis, PersistedRoutePreflightError, PersistedRouteSignaturePending,
    PersistedRouteVerificationError, RouteSeedOutcome, VerifiedPersistedRoute,
};
use crate::persistence::PersistedRouteRows;
use crate::storage::StorageLayout;

use super::node_facade::{RouteSeedProgress, RouteSeedReport};

const MIN_PARALLEL_ROUTE_VERIFICATIONS: usize = 64;
const ROUTE_VERIFICATION_BATCH_SIZE: usize = 16;

enum PreparedRoute {
    SignaturePending,
    Refused(PersistedRoutePreflightError),
    MalformedSnapshot,
}

struct VerificationExecution<'a> {
    routes: Vec<Result<VerifiedPersistedRoute<'a>, PersistedRouteVerificationError>>,
    workers: usize,
}

pub(super) struct RouteRestoreExecution {
    report: RouteSeedReport,
    verification_workers: usize,
}

impl RouteRestoreExecution {
    pub(super) fn into_report(self) -> RouteSeedReport {
        let _ = self.verification_workers;
        self.report
    }

    #[cfg(test)]
    fn verification_workers(&self) -> usize {
        self.verification_workers
    }
}

pub(super) fn seed_persisted_routes<S: StorageLayout>(
    engine: &mut EngineState<S>,
    rows: PersistedRouteRows<'_>,
    now: InstantMillis,
    workers: Option<NonZeroUsize>,
    mut progress: impl FnMut(RouteSeedProgress),
) -> RouteRestoreExecution {
    let total_count = rows.remaining_row_count();
    progress(RouteSeedProgress {
        processed_count: 0,
        total_count,
    });
    let Some(workers) = workers else {
        return seed_inline(engine, rows, now, total_count, progress);
    };
    if workers.get() == 1
        || usize::try_from(total_count).unwrap_or(usize::MAX) < MIN_PARALLEL_ROUTE_VERIFICATIONS
    {
        return seed_inline(engine, rows, now, total_count, progress);
    }
    seed_parallel(engine, rows, now, total_count, workers, progress)
}

fn seed_inline<S: StorageLayout>(
    engine: &mut EngineState<S>,
    rows: PersistedRouteRows<'_>,
    now: InstantMillis,
    total_count: u32,
    mut progress: impl FnMut(RouteSeedProgress),
) -> RouteRestoreExecution {
    let mut report = RouteSeedReport::default();
    for (processed_index, row) in rows.enumerate() {
        let processed_count = u32::try_from(processed_index)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let Ok(row) = row else {
            report.refused_count = report.refused_count.saturating_add(1);
            progress(RouteSeedProgress {
                processed_count,
                total_count,
            });
            break;
        };
        record_outcome(&mut report, engine.seed_route(&row, now));
        progress(RouteSeedProgress {
            processed_count,
            total_count,
        });
    }
    RouteRestoreExecution {
        report,
        verification_workers: 0,
    }
}

fn seed_parallel<S: StorageLayout>(
    engine: &mut EngineState<S>,
    rows: PersistedRouteRows<'_>,
    now: InstantMillis,
    total_count: u32,
    workers: NonZeroUsize,
    mut progress: impl FnMut(RouteSeedProgress),
) -> RouteRestoreExecution {
    let row_capacity = usize::try_from(total_count).unwrap_or_default();
    let mut pending = Vec::with_capacity(row_capacity);
    let mut prepared = Vec::with_capacity(row_capacity);
    let mut preflight_completed = 0u32;
    for row in rows {
        let Ok(row) = row else {
            prepared.push(PreparedRoute::MalformedSnapshot);
            preflight_completed = preflight_completed.saturating_add(1);
            break;
        };
        match engine.prepare_persisted_route(row) {
            Ok(candidate) => {
                pending.push(candidate);
                prepared.push(PreparedRoute::SignaturePending);
            }
            Err(error) => {
                prepared.push(PreparedRoute::Refused(error));
                preflight_completed = preflight_completed.saturating_add(1);
            }
        }
    }
    if preflight_completed > 0 {
        progress(RouteSeedProgress {
            processed_count: preflight_completed,
            total_count,
        });
    }
    let verification = if pending.len() < MIN_PARALLEL_ROUTE_VERIFICATIONS {
        verify_inline(&pending, |completed| {
            progress(RouteSeedProgress {
                processed_count: preflight_completed.saturating_add(completed),
                total_count,
            });
        })
    } else {
        verify_parallel(&pending, workers, |completed| {
            progress(RouteSeedProgress {
                processed_count: preflight_completed.saturating_add(completed),
                total_count,
            });
        })
    };
    let mut report = RouteSeedReport::default();
    let mut verified = verification.routes.into_iter();
    for route in prepared {
        match route {
            PreparedRoute::SignaturePending => match verified.next() {
                Some(Ok(route)) => {
                    record_outcome(&mut report, engine.seed_verified_route(route, now))
                }
                Some(Err(PersistedRouteVerificationError::InvalidSignature)) | None => {
                    report.refused_count = report.refused_count.saturating_add(1);
                }
            },
            PreparedRoute::Refused(
                PersistedRoutePreflightError::DestinationMismatch
                | PersistedRoutePreflightError::BlackholedIdentity,
            )
            | PreparedRoute::MalformedSnapshot => {
                report.refused_count = report.refused_count.saturating_add(1);
            }
        }
    }
    RouteRestoreExecution {
        report,
        verification_workers: verification.workers,
    }
}

fn verify_inline<'a>(
    pending: &'a [PersistedRouteSignaturePending<'a>],
    mut progress: impl FnMut(u32),
) -> VerificationExecution<'a> {
    let mut completed = 0u32;
    let routes = pending
        .iter()
        .map(|route| {
            let verified = route.verify();
            completed = completed.saturating_add(1);
            progress(completed);
            verified
        })
        .collect();
    VerificationExecution { routes, workers: 0 }
}

fn verify_parallel<'a>(
    pending: &'a [PersistedRouteSignaturePending<'a>],
    requested_workers: NonZeroUsize,
    mut progress: impl FnMut(u32),
) -> VerificationExecution<'a> {
    let batch_count = pending.len().div_ceil(ROUTE_VERIFICATION_BATCH_SIZE);
    let worker_limit = requested_workers.get().min(batch_count);
    let next = AtomicUsize::new(0);
    let (results_tx, results_rx) = mpsc::channel();
    let mut slots = (0..pending.len()).map(|_| None).collect::<Vec<_>>();
    let mut completed = 0u32;
    let workers = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_limit);
        for worker_index in 0..worker_limit {
            let results_tx = results_tx.clone();
            let next = &next;
            let spawned = std::thread::Builder::new()
                .name(format!("prns-route-verify-{worker_index}"))
                .spawn_scoped(scope, move || loop {
                    let start = next.fetch_add(ROUTE_VERIFICATION_BATCH_SIZE, Ordering::Relaxed);
                    if start >= pending.len() {
                        break;
                    }
                    let end = start
                        .saturating_add(ROUTE_VERIFICATION_BATCH_SIZE)
                        .min(pending.len());
                    let batch = (start..end)
                        .map(|index| (index, pending[index].verify()))
                        .collect::<Vec<_>>();
                    if results_tx.send(batch).is_err() {
                        break;
                    }
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        drop(results_tx);
        let worker_count = handles.len();
        for batch in results_rx {
            let mut landed = 0u32;
            for (index, result) in batch {
                let Some(slot) = slots.get_mut(index) else {
                    continue;
                };
                if slot.is_none() {
                    *slot = Some(result);
                    landed = landed.saturating_add(1);
                }
            }
            if landed > 0 {
                completed = completed.saturating_add(landed);
                progress(completed);
            }
        }
        for handle in handles {
            let _ = handle.join();
        }
        worker_count
    });
    let routes = slots
        .into_iter()
        .enumerate()
        .map(|(index, result)| match result {
            Some(result) => result,
            None => {
                let result = pending[index].verify();
                completed = completed.saturating_add(1);
                progress(completed);
                result
            }
        })
        .collect();
    VerificationExecution { routes, workers }
}

fn record_outcome(report: &mut RouteSeedReport, outcome: RouteSeedOutcome) {
    match outcome {
        RouteSeedOutcome::Seeded => {
            report.seeded_count = report.seeded_count.saturating_add(1);
        }
        RouteSeedOutcome::RefusedDestinationMismatch
        | RouteSeedOutcome::RefusedBlackholedIdentity
        | RouteSeedOutcome::RefusedInvalidSignature => {
            report.refused_count = report.refused_count.saturating_add(1);
        }
        RouteSeedOutcome::AlreadyPresent
        | RouteSeedOutcome::TableFull
        | RouteSeedOutcome::AppDataArenaFull => {
            report.dropped_count = report.dropped_count.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentityHash;
    use crate::interfaces::{AttachedInterfaces, InterfaceId};
    use crate::persistence::{
        read_routing_table_snapshot, routing_table_snapshot_len, write_routing_table_snapshot,
    };
    use crate::routing::announce::{Announce, AnnounceId, DottedNameHash};
    use crate::routing::routes::RouteEntry;
    use crate::routing::{
        AnnounceIdRing, BlackholeExpiry, BlackholedIdentity, NextHop, PersistedRouteRow,
        RouteResponsiveness,
    };
    use crate::storage::GrowableHeap;
    use crate::wire::DestinationHash;

    fn signed_row(index: u16, signer_byte: u8) -> PersistedRouteRow<'static> {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[signer_byte; 64]);
        let mut dotted_name_hash = [0u8; 10];
        dotted_name_hash[..2].copy_from_slice(&index.to_le_bytes());
        let announce = Announce::build_signed(
            &signer,
            DottedNameHash::new(dotted_name_hash),
            AnnounceId::from_wire([index as u8; 10]),
            None,
            b"",
        )
        .unwrap();
        PersistedRouteRow {
            destination: announce.destination,
            entry: RouteEntry {
                hops: (index % 8) as u8,
                learned_at: InstantMillis(u64::from(index) * 10),
                last_relayed_at: InstantMillis(u64::from(index) * 10 + 1),
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: InterfaceId::new([index as u8; 8]),
                next_hop: NextHop::Direct,
            },
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            app_data: b"",
            announce_id_ring: AnnounceIdRing::Wire(&[]),
        }
    }

    fn encoded_rows(rows: &[PersistedRouteRow<'_>]) -> Vec<u8> {
        let mut bytes = vec![0u8; routing_table_snapshot_len(rows.iter().cloned())];
        let written = write_routing_table_snapshot(rows.iter().cloned(), &mut bytes).unwrap();
        bytes.truncate(written);
        bytes
    }

    fn encoded_engine(engine: &EngineState<GrowableHeap>) -> Vec<u8> {
        let mut bytes = vec![0u8; routing_table_snapshot_len(engine.persisted_route_rows())];
        let written =
            write_routing_table_snapshot(engine.persisted_route_rows(), &mut bytes).unwrap();
        bytes.truncate(written);
        bytes
    }

    fn blackhole(engine: &mut EngineState<GrowableHeap>, identity: IdentityHash) {
        engine
            .blackhole_identity(
                BlackholedIdentity {
                    identity,
                    source: IdentityHash::new([0xC4; 16]),
                    expiry: BlackholeExpiry::Indefinite,
                    reason: None::<&str>,
                },
                AttachedInterfaces::new(&[]),
                &mut |_| {},
            )
            .outcome
            .unwrap();
    }

    #[test]
    fn parallel_verification_preserves_serial_report_and_route_order() {
        let mut rows = (0..96)
            .map(|index| signed_row(index, 0x71))
            .collect::<Vec<_>>();
        rows.insert(17, rows[3].clone());
        let mut invalid_signature = signed_row(200, 0x71);
        invalid_signature.signature.0[0] ^= 0x01;
        rows.insert(31, invalid_signature);
        let mut destination_mismatch = signed_row(201, 0x71);
        destination_mismatch.destination = DestinationHash::new([0xD5; 16]);
        rows.insert(47, destination_mismatch);
        let blackholed = signed_row(202, 0x72);
        let blackholed_identity = blackholed.public_keys.identity_hash();
        rows.insert(63, blackholed);
        let bytes = encoded_rows(&rows);

        let mut serial_engine = EngineState::<GrowableHeap>::default();
        let mut parallel_engine = EngineState::<GrowableHeap>::default();
        blackhole(&mut serial_engine, blackholed_identity);
        blackhole(&mut parallel_engine, blackholed_identity);
        let mut serial_progress = Vec::new();
        let serial = seed_persisted_routes(
            &mut serial_engine,
            read_routing_table_snapshot(&bytes).unwrap(),
            InstantMillis(1_000),
            None,
            |progress| serial_progress.push(progress),
        );
        let mut parallel_progress = Vec::new();
        let parallel = seed_persisted_routes(
            &mut parallel_engine,
            read_routing_table_snapshot(&bytes).unwrap(),
            InstantMillis(1_000),
            NonZeroUsize::new(4),
            |progress| parallel_progress.push(progress),
        );

        assert_eq!(serial.verification_workers(), 0);
        assert_eq!(parallel.verification_workers(), 4);
        assert_eq!(serial.report, parallel.report);
        assert_eq!(
            parallel.report,
            RouteSeedReport {
                seeded_count: 96,
                refused_count: 3,
                dropped_count: 1,
            }
        );
        assert_eq!(
            encoded_engine(&serial_engine),
            encoded_engine(&parallel_engine)
        );
        assert_eq!(serial_progress.first(), parallel_progress.first());
        assert_eq!(serial_progress.last(), parallel_progress.last());
        assert!(parallel_progress
            .windows(2)
            .all(|pair| pair[0].processed_count <= pair[1].processed_count));
    }
}
