use std::path::Path;

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

/// Load the node's persistent transport identity, seeding one on first boot.
///
/// RNS persists a shared instance's identity as the raw X25519 ‖ Ed25519 private key at
/// `<storage_dir>/transport_identity`, and derives the control-RPC key as its SHA-256. The daemon
/// honors an existing file untouched (so it keeps one stable identity across restarts and a stock
/// client computes the same `rpc_key`); an absent one means this is the first instance on the host,
/// so a fresh OS-CSPRNG identity is generated and written, owning the identity as a shared instance
/// does. Handed onward through a [`Zeroizing`] buffer so it is wiped once construction copies it in.
#[must_use]
pub fn load_or_seed_transport_identity(
    storage_dir: &Path,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        let _ = storage_dir;
        key[..32].fill(0x22);
        key[32..].fill(0x11);
    }

    #[cfg(not(feature = "fixture-identity"))]
    {
        let path = storage_dir.join("transport_identity");
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == IDENTITY_SECRET_KEY_LEN => key.copy_from_slice(&bytes),
            _ => {
                getrandom::getrandom(&mut *key)
                    .expect("OS CSPRNG must provide identity key material");
                let _ = std::fs::create_dir_all(storage_dir);
                let _ = std::fs::write(&path, &key[..]);
            }
        }
    }

    key
}
