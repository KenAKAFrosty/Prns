use personal_rns::config::BlackholeExchangePlan;
use personal_rns::identity::vault::FileVault;
use personal_rns::identity::IdentityHash;
use personal_rns::persistence::FileStore;
use personal_rns::runtime::request_router::RouteSet;
use personal_rns::runtime::{PrnsEvent, PrnsNode};
use personal_rns::shared_instance::RnsBlackholeFiles;
use personal_rns::storage::StorageLayout;
use personal_rns::units::InstantMillis;

use crate::services::DaemonRequestState;
use crate::startup_progress::StateRestoreProgress;

pub(crate) struct RestoreInputs<'a> {
    pub(crate) store: &'a FileStore,
    pub(crate) vault: &'a FileVault,
    pub(crate) blackhole_files: &'a RnsBlackholeFiles,
    pub(crate) blackhole_exchange: &'a BlackholeExchangePlan,
    pub(crate) local_identity: IdentityHash,
    pub(crate) timeline_origin: InstantMillis,
    pub(crate) progress: Option<StateRestoreProgress>,
}

pub(crate) fn restore<R, F, S>(
    node: &mut PrnsNode<DaemonRequestState, R, F, S>,
    mut inputs: RestoreInputs<'_>,
) where
    R: RouteSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let mut restored_blackholes = match inputs
        .blackhole_files
        .load_local(inputs.local_identity, inputs.timeline_origin)
    {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(event = "blackhole_restore_failed", error = %error);
            Vec::new()
        }
    };
    for source in inputs.blackhole_exchange.sources() {
        match inputs
            .blackhole_files
            .load_source(*source, inputs.timeline_origin)
        {
            Ok(entries) => restored_blackholes.extend(entries),
            Err(error) => tracing::warn!(
                event = "blackhole_source_restore_failed",
                source = ?source.as_bytes(),
                error = %error,
            ),
        }
    }
    let blackholes = node.seed_blackholed_identities(restored_blackholes);
    let routes = match inputs.progress.as_mut() {
        Some(progress) => node.seed_routes_from_store_reporting(inputs.store, |route_progress| {
            progress.observe(route_progress);
        }),
        None => node.seed_routes_from_store(inputs.store),
    };
    let destination_identities = node.seed_destination_identities_from_store(inputs.store);
    let tunnels = node.seed_tunnels_from_store(inputs.store);
    let ratchets = node.seed_self_ratchets_from_vault(inputs.vault);
    if let Some(progress) = inputs.progress {
        progress.finish();
    }
    tracing::info!(
        event = "state_restored",
        blackholes = blackholes.seeded_count,
        routes = routes.seeded_count,
        destination_identities = destination_identities.seeded_count,
        tunnels = tunnels.seeded_count,
        ratchets = ratchets.seeded_count,
        refused = blackholes.refused_count
            + routes.refused_count
            + destination_identities.refused_count
            + tunnels.refused_count
            + ratchets.refused_count,
        dropped = blackholes.dropped_count
            + routes.dropped_count
            + destination_identities.dropped_count
            + tunnels.dropped_count
            + ratchets.dropped_count,
    );
}
