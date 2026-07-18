use crate::crypto::ratchets::LastRotated;
use crate::crypto::X25519SecretKey;
use crate::engine::{EngineState, InstantMillis};
use crate::identity::Zeroizing;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::host_protocol::{PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot};

pub(super) fn persisted_state<S: StorageLayout>(
    engine: &EngineState<S>,
    taken_at: InstantMillis,
) -> Option<PersistedStateSnapshot> {
    let mut routing_table = std::vec![0u8; crate::persistence::routing_table_snapshot_len(engine.persisted_route_rows())];
    let mut tunnels = std::vec![
        0u8;
        crate::persistence::tunnels_snapshot_len(engine.persisted_tunnel_rows().count())
    ];
    let mut destination_identities = std::vec![
        0u8;
        crate::persistence::destination_identities_snapshot_len(
            engine.destination_identities(),
        )
    ];

    let (Ok(routes_len), Ok(tunnels_len), Ok(destination_identities_len)) = (
        crate::persistence::write_routing_table_snapshot(
            engine.persisted_route_rows(),
            &mut routing_table,
        ),
        crate::persistence::write_tunnels_snapshot(engine.persisted_tunnel_rows(), &mut tunnels),
        crate::persistence::write_destination_identities_snapshot(
            engine.destination_identities(),
            &mut destination_identities,
        ),
    ) else {
        return None;
    };

    routing_table.truncate(routes_len);
    tunnels.truncate(tunnels_len);
    destination_identities.truncate(destination_identities_len);
    Some(PersistedStateSnapshot {
        routing_table,
        tunnels,
        destination_identities,
        taken_at,
    })
}

pub(super) fn self_ratchets<S: StorageLayout>(engine: &EngineState<S>) -> SelfRatchetsSnapshot {
    let blobs = engine
        .persisted_self_ratchet_rows()
        .filter_map(|(destination, last_rotated, secrets)| {
            seal_self_ratchet(last_rotated, secrets).map(|sealed| (destination, sealed))
        })
        .collect();
    SelfRatchetsSnapshot { blobs }
}

pub(super) fn self_ratchet<S: StorageLayout>(
    engine: &EngineState<S>,
    destination: DestinationHash,
) -> Option<SelfRatchetSnapshot> {
    let (last_rotated, secrets) = engine.persisted_self_ratchet_row(&destination)?;
    let sealed = seal_self_ratchet(last_rotated, secrets)?;
    Some(SelfRatchetSnapshot {
        destination,
        sealed,
    })
}

fn seal_self_ratchet(
    last_rotated: LastRotated,
    secrets: &[X25519SecretKey],
) -> Option<Zeroizing<std::vec::Vec<u8>>> {
    let mut sealed = Zeroizing::new(std::vec![
        0u8;
        crate::persistence::self_ratchets_snapshot_len(secrets.len())
    ]);
    let written =
        crate::persistence::write_self_ratchets_snapshot(last_rotated, secrets, &mut sealed)
            .ok()?;
    sealed.truncate(written);
    Some(sealed)
}
