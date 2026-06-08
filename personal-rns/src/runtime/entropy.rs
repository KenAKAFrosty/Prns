use crate::engine::{RatchetEntropy, SendSingleEntropy};
use crate::routing::announce::SelfAnnounceEntropy;

pub struct UnspentEntropyPool {
    self_announce: Option<SelfAnnounceEntropy>,
    ratchet: Option<RatchetEntropy>,
    send: Option<SendSingleEntropy>,
}

impl UnspentEntropyPool {
    pub const fn empty() -> Self {
        Self {
            self_announce: None,
            ratchet: None,
            send: None,
        }
    }

    #[must_use]
    pub fn checkout_self_announce(&mut self, fill: impl FnOnce(&mut [u8])) -> SelfAnnounceEntropy {
        self.self_announce.take().unwrap_or_else(|| {
            let mut bytes = [0u8; SelfAnnounceEntropy::LEN];
            fill(&mut bytes);
            SelfAnnounceEntropy::new(bytes)
        })
    }

    #[must_use]
    pub fn checkout_ratchet(&mut self, fill: impl FnOnce(&mut [u8])) -> RatchetEntropy {
        self.ratchet.take().unwrap_or_else(|| {
            let mut bytes = [0u8; RatchetEntropy::LEN];
            fill(&mut bytes);
            RatchetEntropy::new(bytes)
        })
    }

    #[must_use]
    pub fn checkout_send_single(&mut self, fill: impl FnOnce(&mut [u8])) -> SendSingleEntropy {
        self.send.take().unwrap_or_else(|| {
            let mut bytes = [0u8; SendSingleEntropy::LEN];
            fill(&mut bytes);
            SendSingleEntropy::new(bytes)
        })
    }

    pub fn restore_self_announce(&mut self, unspent: SelfAnnounceEntropy) {
        self.self_announce = Some(unspent);
    }

    pub fn restore_ratchet(&mut self, unspent: RatchetEntropy) {
        self.ratchet = Some(unspent);
    }

    pub fn restore_send_single(&mut self, unspent: SendSingleEntropy) {
        self.send = Some(unspent);
    }
}

impl Default for UnspentEntropyPool {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::self_ratchets::{
        FixedSelfRatchetColumns, RatchetRotation, SelfRatchets, MIN_RATCHET_ROTATION_INTERVAL_MS,
    };
    use crate::engine::InstantMillis;
    use crate::routing::announce::AnnounceId;

    fn self_announce_of(byte: u8) -> SelfAnnounceEntropy {
        SelfAnnounceEntropy::new([byte; SelfAnnounceEntropy::LEN])
    }

    fn ratchet_of(byte: u8) -> RatchetEntropy {
        RatchetEntropy::new([byte; RatchetEntropy::LEN])
    }

    fn announce_id_of(entropy: SelfAnnounceEntropy) -> AnnounceId {
        AnnounceId::mint(entropy, InstantMillis(7))
    }

    fn minted_key_of(
        ratchets: &mut SelfRatchets<FixedSelfRatchetColumns<1, 3>>,
        at: InstantMillis,
        entropy: RatchetEntropy,
    ) -> crate::routing::announce::RatchetKey {
        let destination = crate::wire::DestinationHash::new([1; 16]);
        assert!(matches!(
            ratchets.rotate_if_due(&destination, at, entropy),
            RatchetRotation::Rotated
        ));
        ratchets.newest_ratchet_key(&destination).unwrap()
    }

    #[test]
    fn an_empty_pool_hands_back_the_fresh_unit() {
        let mut pool = UnspentEntropyPool::empty();
        let out = pool.checkout_self_announce(|bytes| bytes.fill(0xBB));
        assert_eq!(announce_id_of(out), announce_id_of(self_announce_of(0xBB)));
    }

    #[test]
    fn a_restored_survivor_wins_over_the_fresh_unit() {
        let mut pool = UnspentEntropyPool::empty();
        pool.restore_self_announce(self_announce_of(0xAA));
        let out = pool.checkout_self_announce(|bytes| bytes.fill(0xBB));
        assert_eq!(announce_id_of(out), announce_id_of(self_announce_of(0xAA)));
    }

    #[test]
    fn checkout_drains_the_slot_so_the_next_cycle_gets_fresh() {
        let mut pool = UnspentEntropyPool::empty();
        pool.restore_self_announce(self_announce_of(0xAA));
        let _first = pool.checkout_self_announce(|bytes| bytes.fill(0xBB));
        let second = pool.checkout_self_announce(|bytes| bytes.fill(0xCC));
        assert_eq!(
            announce_id_of(second),
            announce_id_of(self_announce_of(0xCC))
        );
    }

    #[test]
    fn the_ratchet_slot_round_trips_byte_faithfully() {
        let mut pool = UnspentEntropyPool::empty();
        pool.restore_ratchet(ratchet_of(0x55));
        let out = pool.checkout_ratchet(|bytes| bytes.fill(0x66));

        let mut minted = SelfRatchets::<FixedSelfRatchetColumns<1, 3>>::default();
        minted
            .track(crate::wire::DestinationHash::new([1; 16]))
            .unwrap();
        let mut expected = SelfRatchets::<FixedSelfRatchetColumns<1, 3>>::default();
        expected
            .track(crate::wire::DestinationHash::new([1; 16]))
            .unwrap();
        assert_eq!(
            minted_key_of(&mut minted, InstantMillis(0), out),
            minted_key_of(&mut expected, InstantMillis(0), ratchet_of(0x55)),
        );

        let next = pool.checkout_ratchet(|bytes| bytes.fill(0x66));
        assert_eq!(
            minted_key_of(
                &mut minted,
                InstantMillis(MIN_RATCHET_ROTATION_INTERVAL_MS),
                next
            ),
            minted_key_of(
                &mut expected,
                InstantMillis(MIN_RATCHET_ROTATION_INTERVAL_MS),
                ratchet_of(0x66)
            ),
        );
    }

    #[test]
    fn each_slot_is_independent() {
        let mut pool = UnspentEntropyPool::empty();
        pool.restore_ratchet(ratchet_of(0x55));

        let self_announce_entropy = pool.checkout_self_announce(|bytes| bytes.fill(0xBB));
        assert_eq!(
            announce_id_of(self_announce_entropy),
            announce_id_of(self_announce_of(0xBB))
        );

        let mut minted = SelfRatchets::<FixedSelfRatchetColumns<1, 3>>::default();
        minted
            .track(crate::wire::DestinationHash::new([1; 16]))
            .unwrap();
        let mut expected = SelfRatchets::<FixedSelfRatchetColumns<1, 3>>::default();
        expected
            .track(crate::wire::DestinationHash::new([1; 16]))
            .unwrap();
        let ratchet = pool.checkout_ratchet(|bytes| bytes.fill(0x66));
        assert_eq!(
            minted_key_of(&mut minted, InstantMillis(0), ratchet),
            minted_key_of(&mut expected, InstantMillis(0), ratchet_of(0x55)),
        );
    }
}
