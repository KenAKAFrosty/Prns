mod host;

pub use host::{
    DefaultLocationError, FlushFailurePolicy, NodePersistence, PersistenceEvent,
    PersistenceFlushStatus, PersistenceIntent, PersistenceRestoreReport, PersistenceTrigger,
    PersistenceWorker, SaveOnLearn, SaveOnLearnWiring,
};

use tokio::sync::oneshot;

use crate::crypto::ratchets::SeedSelfRatchetsOutcome;
use crate::engine::{BlackholeSeedReport, DestinationIdentitySeedOutcome, InstantMillis};
use crate::identity::vault::IdentityVault;
use crate::identity::Zeroizing;
use crate::interfaces::AttachedInterfaces;
use crate::manifold::driver::{
    HostCommand, PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot,
};
use crate::manifold::Host;
use crate::persistence::{
    read_destination_identities_snapshot, read_routing_table_snapshot, read_self_ratchets_snapshot,
    read_timebase_snapshot, read_tunnels_snapshot, snapshot_fingerprint, write_timebase_snapshot,
    PersistedStore, SnapshotFingerprint, SnapshotRegion, TIMEBASE_SNAPSHOT_LEN,
};
use crate::routing::tunnel::SeedTunnelOutcome;
use crate::routing::BlackholedIdentity;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;
use prns_runtime::runtime::persistence_snapshots::self_ratchet_identity_label;

use super::super::request_endpoints::RequestEndpointSet;
use super::super::PrnsEvent;
use super::{PrnsNode, PrnsNodeHandle};

pub(super) const MAX_BOOT_RECORD_LEN: usize = 64 * 1024 * 1024;

pub(super) fn try_zeroed_buffer(len: usize) -> Option<Vec<u8>> {
    if len > MAX_BOOT_RECORD_LEN {
        return None;
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(len).ok()?;
    buffer.resize(len, 0);
    Some(buffer)
}

impl PrnsNodeHandle {
    /// A consistent image of every persisted region, serialized on the manifold — the one place a consistent view exists — with the engine instant it was taken at. `None` once the node has stopped.
    pub async fn snapshot_persisted_state(&self) -> Option<PersistedStateSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotPersistedState { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    /// One full flush, unconditionally rewriting every region — right for a shutdown handler; an interval loop wants [`flush_changed_to_store`](Self::flush_changed_to_store).
    pub async fn flush_to_store<P: PersistedStore>(
        &self,
        store: &mut P,
    ) -> Result<InstantMillis, FlushError<P::Error>> {
        self.prepare_flush()
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?
            .commit_to_store(store, &mut FlushMark::default())
            .map(|report| report.high_water)
    }

    pub async fn prepare_flush(&self) -> Result<PreparedFlush, PrepareFlushError> {
        self.snapshot_persisted_state()
            .await
            .map(|snapshot| PreparedFlush { snapshot })
            .ok_or(PrepareFlushError::NodeStopped)
    }

    /// The interval-cadence flush: a region whose sealed image fingerprints the same as `mark`'s last landed flush is skipped, so a quiet node's tick writes 22 bytes of timebase and nothing else. The timebase always writes — it is the restored timeline's rollback floor and it advances with uptime even while the tables sit still — and it lands before the region images it stamps: a crash between them leaves a newer high-water over older rows, which only over-ages the restored timeline, where the reverse order could strand rows in a wall-less boot's future, never expiring.
    #[allow(clippy::expect_used)]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "prns.persistence.flush", level = "debug", skip_all)
    )]
    pub async fn flush_changed_to_store<P: PersistedStore>(
        &self,
        store: &mut P,
        mark: &mut FlushMark,
    ) -> Result<FlushReport, FlushError<P::Error>> {
        self.prepare_flush()
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?
            .commit_to_store(store, mark)
    }

    /// Every tracked destination's sealed self-ratchet record, serialized on the manifold. `None` once the node has stopped.
    pub async fn snapshot_self_ratchets(&self) -> Option<SelfRatchetsSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotSelfRatchets { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    pub async fn snapshot_self_ratchet(
        &self,
        destination: DestinationHash,
    ) -> Result<Option<SelfRatchetSnapshot>, PrepareFlushError> {
        let (reply, snapshot) = oneshot::channel();
        self.commands
            .send(HostCommand::SnapshotSelfRatchet { destination, reply })
            .map_err(|_| PrepareFlushError::NodeStopped)?;
        snapshot.await.map_err(|_| PrepareFlushError::NodeStopped)
    }

    /// Flush every destination's self-ratchet record to `vault`, returning how many landed. Ratchet secrets never touch a [`PersistedStore`]: the vault is where the identity secret itself lives, so the record inherits its protections.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "prns.persistence.flush_ratchets", level = "debug", skip_all)
    )]
    pub async fn flush_ratchets_to_vault<V: IdentityVault>(
        &self,
        vault: &mut V,
    ) -> Result<u32, FlushError<V::Error>> {
        let snapshot = self
            .snapshot_self_ratchets()
            .await
            .ok_or(FlushError::NodeStopped)?;
        snapshot.store_into(vault).map_err(FlushError::Store)
    }

    pub async fn flush_ratchet_to_vault<V: IdentityVault>(
        &self,
        destination: DestinationHash,
        vault: &mut V,
    ) -> Result<bool, FlushError<V::Error>> {
        let snapshot = self
            .snapshot_self_ratchet(destination)
            .await
            .map_err(|PrepareFlushError::NodeStopped| FlushError::NodeStopped)?;
        match snapshot {
            Some(snapshot) => snapshot
                .store_into(vault)
                .map(|()| true)
                .map_err(FlushError::Store),
            None => Ok(false),
        }
    }
}

impl<St, R, F, S: StorageLayout> PrnsNode<St, R, F, S>
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Must precede [`seed_routes_from_store`](Self::seed_routes_from_store) so restored rows sit in this boot's past.
    #[must_use]
    pub fn with_timeline_origin(mut self, origin: InstantMillis) -> Self {
        self.host.set_timeline_origin(origin);
        self
    }

    pub fn seed_blackholed_identities<Reason: AsRef<str>>(
        &mut self,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> BlackholeSeedReport {
        self.node
            .engine
            .seed_blackholed_identities(
                entries,
                self.host.now(),
                AttachedInterfaces::new(&[]),
                &mut |_| {},
            )
            .report
    }

    /// Boot-restore before [`run`](Self::run): every stored row re-verifies its signature and address binding before landing, and lands with the departed grace on its interface. Refusals and drops are counted, never fatal — a damaged snapshot costs rows, not the boot.
    pub fn seed_routes_from_store(&mut self, store: &impl PersistedStore) -> RouteSeedReport {
        self.seed_routes_from_store_reporting(store, |_| {})
    }

    pub fn seed_routes_from_store_reporting(
        &mut self,
        store: &impl PersistedStore,
        progress: impl FnMut(RouteSeedProgress),
    ) -> RouteSeedReport {
        let mut report = RouteSeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::RoutingTable) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::RoutingTable, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_routing_table_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        let now = self.host.now();
        let workers = self.crypto_pool.resolved_worker_count();
        super::super::route_restore::seed_persisted_routes(
            &mut self.node.engine,
            rows,
            now,
            workers,
            progress,
        )
        .into_report()
    }

    pub fn seed_destination_identities_from_store(
        &mut self,
        store: &impl PersistedStore,
    ) -> DestinationIdentitySeedReport {
        let mut report = DestinationIdentitySeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::DestinationIdentities) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::DestinationIdentities, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_destination_identities_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        let now = self.host.now();
        for row in rows {
            let Ok(row) = row else {
                report.refused_count += 1;
                break;
            };
            match self.node.engine.seed_destination_identity(row, now) {
                DestinationIdentitySeedOutcome::Seeded => report.seeded_count += 1,
                DestinationIdentitySeedOutcome::RefusedPublicKeyChanged => {
                    report.refused_count += 1;
                }
                DestinationIdentitySeedOutcome::Replaced
                | DestinationIdentitySeedOutcome::Expired
                | DestinationIdentitySeedOutcome::CapacityExhausted => report.dropped_count += 1,
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each seeded tunnel warms its stored interface's routes until the peer's next synthesize, which repoints them onto the live connection — this is what re-claims routes whose interface id never comes back, an ephemeral client's reconnect being the canonical case. Refusals are counted, never fatal.
    pub fn seed_tunnels_from_store(&mut self, store: &impl PersistedStore) -> TunnelSeedReport {
        let mut report = TunnelSeedReport::default();
        let Ok(Some(stored_len)) = store.stored_len(SnapshotRegion::Tunnels) else {
            return report;
        };
        let Some(mut buf) = try_zeroed_buffer(stored_len) else {
            report.refused_count = report.refused_count.saturating_add(1);
            return report;
        };
        let Ok(Some(bytes)) = store.load(SnapshotRegion::Tunnels, &mut buf) else {
            return report;
        };
        let Ok(rows) = read_tunnels_snapshot(bytes) else {
            report.refused_count += 1;
            return report;
        };
        for row in rows {
            match self.node.engine.seed_tunnel(row) {
                SeedTunnelOutcome::Seeded => report.seeded_count += 1,
                SeedTunnelOutcome::AlreadyPresent | SeedTunnelOutcome::TableFull => {
                    report.dropped_count += 1;
                }
            }
        }
        report
    }

    /// Boot-restore before [`run`](Self::run): each ratcheted destination the recipe registered reloads its rotation clock and retained secrets from the vault, so singles peers encrypted toward pre-reboot ratchets decrypt again. Refusals are counted, never fatal.
    pub fn seed_self_ratchets_from_vault<V: IdentityVault>(
        &mut self,
        vault: &V,
    ) -> RatchetSeedReport {
        let mut report = RatchetSeedReport::default();
        let destinations: Vec<DestinationHash> = self
            .node
            .engine
            .persisted_self_ratchet_rows()
            .map(|(destination, _, _)| destination)
            .collect();
        for destination in destinations {
            let label = self_ratchet_identity_label(&destination);
            let Ok(Some(stored_len)) = vault.stored_blob_len(&label) else {
                continue;
            };
            let Some(buf) = try_zeroed_buffer(stored_len) else {
                report.refused_count = report.refused_count.saturating_add(1);
                continue;
            };
            let mut buf = Zeroizing::new(buf);
            let Ok(Some(bytes)) = vault.load_blob(&label, &mut buf) else {
                continue;
            };
            let Ok(record) = read_self_ratchets_snapshot(bytes) else {
                report.refused_count += 1;
                continue;
            };
            match self.node.engine.seed_self_ratchets(
                &destination,
                record.last_rotated,
                record
                    .secrets_newest_first()
                    .collect::<Vec<_>>()
                    .into_iter(),
            ) {
                SeedSelfRatchetsOutcome::Seeded => report.seeded_count += 1,
                SeedSelfRatchetsOutcome::AlreadyMinted | SeedSelfRatchetsOutcome::Untracked => {
                    report.dropped_count += 1;
                }
            }
        }
        report
    }
}

/// The boot origin for a wall-clocked host: wall time floored by the stored high-water, so a rolled-back clock can never restart the timeline under persisted rows. Absent or unreadable snapshots fall back gracefully — boot never blocks on storage health.
pub fn boot_timeline_origin(store: &impl PersistedStore) -> InstantMillis {
    let wall_now = wall_clock_timeline_origin().0;
    let mut buf = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let high_water = match store.load(SnapshotRegion::Timebase, &mut buf) {
        Ok(Some(bytes)) => read_timebase_snapshot(bytes)
            .map(|high_water| high_water.0)
            .unwrap_or(0),
        _ => 0,
    };
    InstantMillis(wall_now.max(high_water))
}

pub fn wall_clock_timeline_origin() -> InstantMillis {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    InstantMillis(millis)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteSeedProgress {
    pub processed_count: u32,
    pub total_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DestinationIdentitySeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TunnelSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RatchetSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

pub struct PreparedFlush {
    snapshot: PersistedStateSnapshot,
}

impl PreparedFlush {
    #[allow(clippy::expect_used)]
    pub fn commit_to_store<P: PersistedStore>(
        self,
        store: &mut P,
        mark: &mut FlushMark,
    ) -> Result<FlushReport, FlushError<P::Error>> {
        let mut timebase = [0u8; TIMEBASE_SNAPSHOT_LEN];
        let timebase_len = write_timebase_snapshot(self.snapshot.taken_at, &mut timebase)
            .expect("TIMEBASE_SNAPSHOT_LEN sizes its own snapshot");
        store
            .store(SnapshotRegion::Timebase, &timebase[..timebase_len])
            .map_err(FlushError::Store)?;
        let routing_table = store_changed_region(
            store,
            SnapshotRegion::RoutingTable,
            &self.snapshot.routing_table,
            &mut mark.routing_table,
        )?;
        let tunnels = store_changed_region(
            store,
            SnapshotRegion::Tunnels,
            &self.snapshot.tunnels,
            &mut mark.tunnels,
        )?;
        let destination_identities = store_changed_region(
            store,
            SnapshotRegion::DestinationIdentities,
            &self.snapshot.destination_identities,
            &mut mark.destination_identities,
        )?;
        Ok(FlushReport {
            high_water: self.snapshot.taken_at,
            routing_table,
            tunnels,
            destination_identities,
        })
    }
}

fn store_changed_region<P: PersistedStore>(
    store: &mut P,
    region: SnapshotRegion,
    sealed: &[u8],
    last_landed: &mut Option<SnapshotFingerprint>,
) -> Result<RegionFlush, FlushError<P::Error>> {
    let fingerprint = snapshot_fingerprint(sealed);
    if fingerprint.is_some() && fingerprint == *last_landed {
        return Ok(RegionFlush::UnchangedSkipped);
    }
    store.store(region, sealed).map_err(FlushError::Store)?;
    *last_landed = fingerprint;
    Ok(RegionFlush::Wrote)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareFlushError {
    NodeStopped,
}

/// The fingerprints of the last flush this mark's owner landed, one per skippable region. A fresh mark knows nothing, so its first flush writes everything once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlushMark {
    pub routing_table: Option<SnapshotFingerprint>,
    pub tunnels: Option<SnapshotFingerprint>,
    pub destination_identities: Option<SnapshotFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionFlush {
    Wrote,
    UnchangedSkipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushReport {
    pub high_water: InstantMillis,
    pub routing_table: RegionFlush,
    pub tunnels: RegionFlush,
    pub destination_identities: RegionFlush,
}

#[derive(Debug)]
pub enum FlushError<E> {
    NodeStopped,
    Store(E),
}

#[cfg(test)]
mod tests;
