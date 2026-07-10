mod impls;

pub use impls::*;

use crate::crypto::{sha256, x25519_public_key, X25519PublicKey, X25519SecretKey};
use crate::engine::InstantMillis;
use crate::routing::announce::RatchetKey;
use crate::wire::DestinationHash;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// RNS 1.3.5 `Destination.RATCHET_INTERVAL`: the minimum time between minting new ratchet keys.
/// Rotation rides the announce. An announce inside the floor re-carries the newest ratchet instead of minting another.
pub const MIN_RATCHET_ROTATION_INTERVAL_MS: u64 = 30 * 60 * 1000;

/// RNS 1.3.5 `Destination.enable_ratchets` / `Destination.enforce_ratchets`.
/// One enum where the reference has two bool flags, so enforcing without ratchets (which would refuse every single packet) is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatchetPolicy {
    NoRatchets,
    Ratcheted,
    RatchetsRequired,
}

pub const RATCHET_ID_LEN: usize = 10;

/// RNS 1.3.5 `Identity._get_ratchet_id`: `full_hash(ratchet_public_bytes)[:NAME_HASH_LENGTH//8]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatchetId([u8; RATCHET_ID_LEN]);

impl RatchetId {
    pub const fn new(bytes: [u8; RATCHET_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub fn of_public_key(public: &X25519PublicKey) -> Self {
        let full = sha256(&public.0);
        let mut id = [0u8; RATCHET_ID_LEN];
        id.copy_from_slice(&full[..RATCHET_ID_LEN]);
        Self(id)
    }

    pub fn of_secret(secret: &X25519SecretKey) -> Self {
        Self::of_public_key(&x25519_public_key(secret))
    }

    pub const fn as_bytes(&self) -> &[u8; RATCHET_ID_LEN] {
        &self.0
    }
}

const RATCHET_SECRET_LEN: usize = 32;

/// A minted ratchet *is* 32 CSPRNG bytes used as an X25519 secret (RNS 1.3.5 `Identity._generate_ratchet`); move-only, deliberately without `Debug`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RatchetEntropy([u8; RATCHET_SECRET_LEN]);

impl RatchetEntropy {
    pub const LEN: usize = RATCHET_SECRET_LEN;

    pub const fn new(bytes: [u8; RATCHET_SECRET_LEN]) -> Self {
        Self(bytes)
    }

    fn into_secret(self) -> X25519SecretKey {
        X25519SecretKey::new(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackRatchetsError {
    TableFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastRotated {
    Never,
    At(InstantMillis),
}

impl LastRotated {
    fn is_rotation_due(self, now: InstantMillis) -> bool {
        match self {
            Self::Never => true,
            Self::At(last) => now.0.saturating_sub(last.0) > MIN_RATCHET_ROTATION_INTERVAL_MS,
        }
    }
}

pub trait SelfRatchetColumns {
    fn capacity(&self) -> usize;
    fn retained_per_destination(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn last_rotated(&self) -> &[LastRotated];
    fn secrets_newest_first(&self, index: usize) -> Option<&[X25519SecretKey]>;

    fn set_last_rotated(&mut self, index: usize, at: InstantMillis);
    fn insert_newest_secret(&mut self, index: usize, secret: X25519SecretKey);
    fn push(&mut self, destination: DestinationHash) -> Result<(), TrackRatchetsError>;
}

#[derive(Default)]
pub struct SelfRatchets<C: SelfRatchetColumns> {
    columns: C,
}

impl<C: SelfRatchetColumns> SelfRatchets<C> {
    pub fn track(&mut self, destination: DestinationHash) -> Result<(), TrackRatchetsError> {
        if self.is_tracked(&destination) {
            return Ok(());
        }
        self.columns.push(destination)
    }

    pub fn is_tracked(&self, destination: &DestinationHash) -> bool {
        self.columns.destinations().contains(destination)
    }

    pub fn has_room(&self) -> bool {
        self.columns.len() < self.columns.capacity()
    }

    /// RNS 1.3.5 `Destination.rotate_ratchets`; a never-rotated row is always due.
    /// Entropy is drawn only once a rotation is actually due.
    pub fn rotate_if_due(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        fill_entropy: &mut impl FnMut(&mut [u8]),
    ) {
        let Some(index) = self.index_of(destination) else {
            return;
        };
        let Some(last_rotated) = self.columns.last_rotated().get(index) else {
            return;
        };
        if !last_rotated.is_rotation_due(now) {
            return;
        }
        let mut entropy_bytes = [0u8; RatchetEntropy::LEN];
        fill_entropy(&mut entropy_bytes);
        self.columns
            .insert_newest_secret(index, RatchetEntropy::new(entropy_bytes).into_secret());
        self.columns.set_last_rotated(index, now);
    }

    pub fn newest_ratchet_key(&self, destination: &DestinationHash) -> Option<RatchetKey> {
        let newest = self.secrets_newest_first(destination).first()?;
        Some(RatchetKey::new(x25519_public_key(newest).0))
    }

    pub fn secrets_newest_first(&self, destination: &DestinationHash) -> &[X25519SecretKey] {
        self.index_of(destination)
            .and_then(|index| self.columns.secrets_newest_first(index))
            .unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }
}

impl<C: SelfRatchetColumns> core::fmt::Debug for SelfRatchets<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SelfRatchets")
            .field("destinations", &self.columns.destinations())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestRatchets = SelfRatchets<FixedSelfRatchetColumns<2, 3>>;

    /// RNS 1.3.5 `Identity._get_ratchet_id(ratchet_public_bytes)` for the `[0x55; 32]` secret.
    #[test]
    fn the_ratchet_id_matches_the_reference_derivation() {
        assert_eq!(
            RatchetId::of_secret(&X25519SecretKey::new([0x55; 32])),
            RatchetId::new([0x11, 0x28, 0xde, 0x8a, 0x3d, 0x96, 0xa5, 0x14, 0xf1, 0x17]),
        );
    }

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn fill(byte: u8) -> impl FnMut(&mut [u8]) {
        move |bytes: &mut [u8]| bytes.fill(byte)
    }

    fn public_of(byte: u8) -> RatchetKey {
        RatchetKey::new(x25519_public_key(&X25519SecretKey::new([byte; 32])).0)
    }

    #[test]
    fn an_untracked_destination_mints_nothing() {
        let mut ratchets = TestRatchets::default();
        assert!(!ratchets.is_tracked(&dest(1)));
        ratchets.rotate_if_due(&dest(1), InstantMillis(0), &mut fill(0x11));
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), None);
        assert!(ratchets.secrets_newest_first(&dest(1)).is_empty());
        assert!(ratchets.is_empty());

        ratchets.track(dest(1)).unwrap();
        ratchets.rotate_if_due(&dest(1), InstantMillis(0), &mut fill(0x11));
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x11)));
    }

    #[test]
    fn the_first_rotation_is_always_due_and_mints_from_the_entropy() {
        let mut ratchets = TestRatchets::default();
        ratchets.track(dest(1)).unwrap();
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), None);

        ratchets.rotate_if_due(&dest(1), InstantMillis(5_000), &mut fill(0x11));
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x11)));
        assert_eq!(ratchets.secrets_newest_first(&dest(1)).len(), 1);
    }

    #[test]
    fn rotation_inside_the_floor_keeps_the_newest_ratchet() {
        let mut ratchets = TestRatchets::default();
        ratchets.track(dest(1)).unwrap();
        ratchets.rotate_if_due(&dest(1), InstantMillis(1_000), &mut fill(0x11));
        ratchets.rotate_if_due(
            &dest(1),
            InstantMillis(1_000 + MIN_RATCHET_ROTATION_INTERVAL_MS),
            &mut fill(0x22),
        );

        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x11)));
        assert_eq!(ratchets.secrets_newest_first(&dest(1)).len(), 1);

        ratchets.rotate_if_due(
            &dest(1),
            InstantMillis(1_000 + MIN_RATCHET_ROTATION_INTERVAL_MS + 1),
            &mut fill(0x22),
        );
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x22)));
    }

    #[test]
    fn rotation_past_the_floor_mints_and_keeps_the_prior_ratchet_behind_it() {
        let mut ratchets = TestRatchets::default();
        ratchets.track(dest(1)).unwrap();
        ratchets.rotate_if_due(&dest(1), InstantMillis(1_000), &mut fill(0x11));
        ratchets.rotate_if_due(
            &dest(1),
            InstantMillis(1_000 + MIN_RATCHET_ROTATION_INTERVAL_MS + 1),
            &mut fill(0x22),
        );

        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x22)));
        let secrets = ratchets.secrets_newest_first(&dest(1));
        assert_eq!(secrets.len(), 2);
        assert_eq!(
            x25519_public_key(&secrets[0]).0,
            *public_of(0x22).as_bytes()
        );
        assert_eq!(
            x25519_public_key(&secrets[1]).0,
            *public_of(0x11).as_bytes()
        );
    }

    #[test]
    fn retention_evicts_the_oldest_ratchet_first() {
        let mut ratchets = TestRatchets::default();
        ratchets.track(dest(1)).unwrap();
        for (round, byte) in [0x11u8, 0x22, 0x33, 0x44].into_iter().enumerate() {
            ratchets.rotate_if_due(
                &dest(1),
                InstantMillis(round as u64 * (MIN_RATCHET_ROTATION_INTERVAL_MS + 1)),
                &mut fill(byte),
            );
        }

        let secrets = ratchets.secrets_newest_first(&dest(1));
        assert_eq!(secrets.len(), 3);
        assert_eq!(
            x25519_public_key(&secrets[0]).0,
            *public_of(0x44).as_bytes()
        );
        assert_eq!(
            x25519_public_key(&secrets[1]).0,
            *public_of(0x33).as_bytes()
        );
        assert_eq!(
            x25519_public_key(&secrets[2]).0,
            *public_of(0x22).as_bytes()
        );
    }

    #[test]
    fn tracking_is_idempotent_and_a_full_table_reports_itself() {
        let mut ratchets = TestRatchets::default();
        assert_eq!(ratchets.track(dest(1)), Ok(()));
        assert_eq!(ratchets.track(dest(1)), Ok(()));
        assert_eq!(ratchets.len(), 1);
        assert!(ratchets.has_room());

        assert_eq!(ratchets.track(dest(2)), Ok(()));
        assert!(!ratchets.has_room());
        assert_eq!(ratchets.track(dest(3)), Err(TrackRatchetsError::TableFull));
        assert_eq!(ratchets.len(), 2);
    }

    #[test]
    fn each_destination_rotates_independently() {
        let mut ratchets = TestRatchets::default();
        ratchets.track(dest(1)).unwrap();
        ratchets.track(dest(2)).unwrap();
        ratchets.rotate_if_due(&dest(1), InstantMillis(1_000), &mut fill(0x11));

        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x11)));
        assert_eq!(ratchets.newest_ratchet_key(&dest(2)), None);

        ratchets.rotate_if_due(&dest(2), InstantMillis(1_000), &mut fill(0x22));
        assert_eq!(ratchets.newest_ratchet_key(&dest(1)), Some(public_of(0x11)));
        assert_eq!(ratchets.newest_ratchet_key(&dest(2)), Some(public_of(0x22)));
    }
}
