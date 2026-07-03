use std::path::Path;

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::runtime::{generate_identity_secret, load_or_create_identity_secret};

/// Load the node's persistent transport identity, seeding one on first boot.
///
/// RNS persists a shared instance's identity as the raw X25519 ‖ Ed25519 private key at
/// `<storage_dir>/transport_identity`, and derives the control-RPC key as its SHA-256. The daemon
/// honors an existing file untouched (so it keeps one stable identity across restarts and a stock
/// client computes the same `rpc_key`); an absent one means this is the first instance on the host,
/// so a fresh OS-CSPRNG identity is generated and written, owning the identity as a shared instance
/// does. A file the daemon cannot read or persist is reported loud, and the node runs on a fresh
/// in-memory identity for this boot only.
#[must_use]
pub fn load_or_seed_transport_identity(
    storage_dir: &Path,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    if cfg!(feature = "fixture-identity") {
        let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
        key[..32].fill(0x22);
        key[32..].fill(0x11);
        return key;
    }
    match load_or_create_identity_secret(&storage_dir.join("transport_identity")) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("RNSD_IDENTITY_EPHEMERAL {error}");
            generate_identity_secret()
        }
    }
}
