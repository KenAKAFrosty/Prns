use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use prns_core::crypto::ratchets::LastRotated;
use prns_core::crypto::X25519SecretKey;
use prns_core::engine::{EngineState, InstantMillis};
use prns_core::identity::vault::{IdentityLabel, IdentityVault};
use prns_core::identity::Zeroizing;
use prns_core::storage::StorageLayout;
use prns_core::wire::DestinationHash;

/// One sealed self-ratchet blob per tracked destination, zeroized on drop.
pub struct SelfRatchetsSnapshot {
    pub blobs: Vec<(DestinationHash, Zeroizing<Vec<u8>>)>,
}

pub struct SelfRatchetSnapshot {
    pub destination: DestinationHash,
    pub sealed: Zeroizing<Vec<u8>>,
}

/// The sealed region images of one snapshot pass and the engine instant they were taken at — the timebase high-water a flush of these images should record.
pub struct PersistedStateSnapshot {
    pub routing_table: Vec<u8>,
    pub tunnels: Vec<u8>,
    pub destination_identities: Vec<u8>,
    pub taken_at: InstantMillis,
}

pub fn snapshot_persisted_state<S: StorageLayout>(
    engine: &EngineState<S>,
    taken_at: InstantMillis,
) -> Option<PersistedStateSnapshot> {
    let mut routing_table =
        vec![
            0u8;
            prns_core::persistence::routing_table_snapshot_len(engine.persisted_route_rows())
        ];
    let mut tunnels =
        vec![
            0u8;
            prns_core::persistence::tunnels_snapshot_len(engine.persisted_tunnel_rows().count())
        ];
    let mut destination_identities = vec![
        0u8;
        prns_core::persistence::destination_identities_snapshot_len(
            engine.destination_identities(),
        )
    ];

    let (Ok(routes_len), Ok(tunnels_len), Ok(destination_identities_len)) = (
        prns_core::persistence::write_routing_table_snapshot(
            engine.persisted_route_rows(),
            &mut routing_table,
        ),
        prns_core::persistence::write_tunnels_snapshot(
            engine.persisted_tunnel_rows(),
            &mut tunnels,
        ),
        prns_core::persistence::write_destination_identities_snapshot(
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

pub fn snapshot_self_ratchets<S: StorageLayout>(engine: &EngineState<S>) -> SelfRatchetsSnapshot {
    let blobs = engine
        .persisted_self_ratchet_rows()
        .filter_map(|(destination, last_rotated, secrets)| {
            seal_self_ratchet(last_rotated, secrets).map(|sealed| (destination, sealed))
        })
        .collect();
    SelfRatchetsSnapshot { blobs }
}

pub fn snapshot_self_ratchet<S: StorageLayout>(
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
) -> Option<Zeroizing<Vec<u8>>> {
    let mut sealed = Zeroizing::new(vec![
        0u8;
        prns_core::persistence::self_ratchets_snapshot_len(
            secrets.len()
        )
    ]);
    let written =
        prns_core::persistence::write_self_ratchets_snapshot(last_rotated, secrets, &mut sealed)
            .ok()?;
    sealed.truncate(written);
    Some(sealed)
}

#[allow(clippy::expect_used)]
#[must_use]
pub fn self_ratchet_identity_label(destination: &DestinationHash) -> IdentityLabel {
    let mut label = String::with_capacity("ratchets.".len() + destination.as_bytes().len() * 2);
    label.push_str("ratchets.");
    for byte in destination.as_bytes() {
        let _ = core::fmt::Write::write_fmt(&mut label, format_args!("{byte:02x}"));
    }
    IdentityLabel::new(&label).expect("a hex destination under a fixed prefix is label-lawful")
}

impl SelfRatchetsSnapshot {
    pub fn store_into<V: IdentityVault>(self, vault: &mut V) -> Result<u32, V::Error> {
        let mut flushed_count = 0u32;
        for (destination, sealed) in self.blobs {
            vault.store_blob(&self_ratchet_identity_label(&destination), &sealed)?;
            flushed_count = flushed_count.saturating_add(1);
        }
        Ok(flushed_count)
    }
}

impl SelfRatchetSnapshot {
    pub fn store_into<V: IdentityVault>(self, vault: &mut V) -> Result<(), V::Error> {
        vault.store_blob(
            &self_ratchet_identity_label(&self.destination),
            &self.sealed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::identity::vault::{IdentitySecretKey, Removal};
    use prns_core::identity::IDENTITY_SECRET_KEY_LEN;

    #[derive(Default)]
    struct CountingVault {
        labels: Vec<String>,
    }

    impl IdentityVault for CountingVault {
        type Error = core::convert::Infallible;

        fn load(&self, _label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
            Ok(None)
        }

        fn store(
            &mut self,
            _label: &IdentityLabel,
            _secret: &[u8; IDENTITY_SECRET_KEY_LEN],
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn remove(&mut self, _label: &IdentityLabel) -> Result<Removal, Self::Error> {
            Ok(Removal::NothingStored)
        }

        fn stored_blob_len(&self, _label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
            Ok(None)
        }

        fn load_blob<'b>(
            &self,
            _label: &IdentityLabel,
            _buf: &'b mut [u8],
        ) -> Result<Option<&'b [u8]>, Self::Error> {
            Ok(None)
        }

        fn store_blob(&mut self, label: &IdentityLabel, _blob: &[u8]) -> Result<(), Self::Error> {
            self.labels.push(label.as_str().into());
            Ok(())
        }
    }

    #[test]
    fn one_ratchet_snapshot_stores_under_its_destination_label() {
        let destination = DestinationHash::new([0x5A; 16]);
        let snapshot = SelfRatchetSnapshot {
            destination,
            sealed: Zeroizing::new(vec![0xA5; 64]),
        };
        let mut vault = CountingVault::default();

        assert_eq!(snapshot.store_into(&mut vault), Ok(()));
        assert_eq!(
            vault.labels,
            vec![self_ratchet_identity_label(&destination).to_string()]
        );
    }
}
