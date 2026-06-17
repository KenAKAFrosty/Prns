//! The links this node carries for others (RNS 1.3.1's `Transport.link_table`).

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::links::LinkId;
use crate::storage::ColumnsFull;
use crate::wire::{DestinationHash, TransportId};

/// RNS 1.3.1 `Transport.LINK_TIMEOUT = Link.STALE_TIME × 1.25`: a switched
/// frame refreshes the row, so only a truly dead link goes idle this long.
pub const TRANSPORTED_LINK_TIMEOUT_MS: u64 = 900_000;

/// RNS 1.3.1 `Transport.extra_link_proof_timeout`: one MTU's airtime on the
/// arrival interface, an allowance for slow last hops. `(8 × 500) / bitrate`
/// seconds, in millis.
#[must_use]
pub fn extra_link_proof_timeout_ms(bitrate_bps: Option<u32>) -> u64 {
    match bitrate_bps {
        Some(bitrate) if bitrate > 0 => 4_000_000u64 / u64::from(bitrate),
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportedLink {
    pub link_id: LinkId,
    pub destination: DestinationHash,
    pub next_hop: Option<TransportId>,
    pub next_hop_interface: InterfaceId,
    pub received_interface: InterfaceId,
    pub taken_hops: u8,
    pub remaining_hops: u8,
    pub validated: bool,
    pub last_active: InstantMillis,
    pub proof_timeout: InstantMillis,
}

impl TransportedLink {
    fn deadline(&self) -> InstantMillis {
        if self.validated {
            InstantMillis(
                self.last_active
                    .0
                    .saturating_add(TRANSPORTED_LINK_TIMEOUT_MS),
            )
        } else {
            self.proof_timeout
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSwitch {
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidateByProofError {
    UnknownLink,
    AlreadyValidated,
    WrongInterface,
    HopMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    UnknownLink,
    NotValidated,
    WrongInterface,
    HopMismatch,
}

pub trait TransportedLinkColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> &[TransportedLink];
    fn entries_mut(&mut self) -> &mut [TransportedLink];
    fn push(&mut self, entry: TransportedLink) -> Result<(), ColumnsFull>;
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct TransportedLinks<C: TransportedLinkColumns> {
    columns: C,
}

impl<C: TransportedLinkColumns> TransportedLinks<C> {
    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.columns
            .entries()
            .iter()
            .position(|entry| entry.link_id == *link_id)
    }

    pub fn track(&mut self, entry: TransportedLink) -> Result<(), ColumnsFull> {
        if self.index_of(&entry.link_id).is_some() {
            return Err(ColumnsFull);
        }
        self.columns.push(entry)
    }

    pub fn entry_for(&self, link_id: &LinkId) -> Option<&TransportedLink> {
        self.index_of(link_id)
            .and_then(|index| self.columns.entries().get(index))
    }

    /// The returning LRPROOF's gate — RNS 1.3.1 transports a proof only when
    /// it arrives over the next hop with exactly the remaining hop count; the
    /// row validates and the proof leaves toward the initiator's side.
    pub fn validate_by_proof(
        &mut self,
        link_id: &LinkId,
        arrived_on: InterfaceId,
        received_hops: u8,
        now: InstantMillis,
    ) -> Result<TransportSwitch, ValidateByProofError> {
        let index = self
            .index_of(link_id)
            .ok_or(ValidateByProofError::UnknownLink)?;
        let entry = self
            .columns
            .entries_mut()
            .get_mut(index)
            .ok_or(ValidateByProofError::UnknownLink)?;
        if entry.validated {
            return Err(ValidateByProofError::AlreadyValidated);
        }
        if arrived_on != entry.next_hop_interface {
            return Err(ValidateByProofError::WrongInterface);
        }
        if received_hops != entry.remaining_hops {
            return Err(ValidateByProofError::HopMismatch);
        }
        entry.validated = true;
        entry.last_active = now;
        Ok(TransportSwitch {
            fire_on: entry.received_interface,
        })
    }

    pub fn switch(
        &mut self,
        link_id: &LinkId,
        arrived_on: InterfaceId,
        received_hops: u8,
        now: InstantMillis,
    ) -> Result<TransportSwitch, SwitchError> {
        let index = self.index_of(link_id).ok_or(SwitchError::UnknownLink)?;
        let entry = self
            .columns
            .entries_mut()
            .get_mut(index)
            .ok_or(SwitchError::UnknownLink)?;
        if !entry.validated {
            return Err(SwitchError::NotValidated);
        }
        let fire_on = if entry.next_hop_interface == entry.received_interface {
            (received_hops == entry.remaining_hops || received_hops == entry.taken_hops)
                .then_some(entry.next_hop_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else if arrived_on == entry.next_hop_interface {
            (received_hops == entry.remaining_hops)
                .then_some(entry.received_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else if arrived_on == entry.received_interface {
            (received_hops == entry.taken_hops)
                .then_some(entry.next_hop_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else {
            return Err(SwitchError::WrongInterface);
        };
        entry.last_active = now;
        Ok(TransportSwitch { fire_on })
    }

    pub fn earliest_deadline(&self) -> Option<InstantMillis> {
        self.columns
            .entries()
            .iter()
            .map(TransportedLink::deadline)
            .min_by_key(|deadline| deadline.0)
    }

    /// Drain one overdue row: an unvalidated row past its proof timeout, or a
    /// validated row idle past the transported-link timeout. Call until `None`.
    pub fn pop_overdue(&mut self, now: InstantMillis) -> Option<TransportedLink> {
        let index = self
            .columns
            .entries()
            .iter()
            .position(|entry| entry.deadline().0 <= now.0)?;
        let entry = *self.columns.entries().get(index)?;
        self.columns.swap_remove(index);
        Some(entry)
    }

    pub fn cull_interface_orphans(&mut self, interface_present: impl Fn(InterfaceId) -> bool) {
        let mut index = 0;
        while index < self.columns.len() {
            let entry = self.columns.entries()[index];
            if interface_present(entry.next_hop_interface)
                && interface_present(entry.received_interface)
            {
                index += 1;
            } else {
                self.columns.swap_remove(index);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestTransported = TransportedLinks<FixedTransportedLinkColumns<3>>;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn entry(link: u8, validated: bool) -> TransportedLink {
        TransportedLink {
            link_id: LinkId::new([link; 16]),
            destination: DestinationHash::new([0xDD; 16]),
            next_hop: Some(TransportId::new([0x77; 16])),
            next_hop_interface: iface(0xB2),
            received_interface: iface(0xA1),
            taken_hops: 1,
            remaining_hops: 1,
            validated,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(9_000),
        }
    }

    #[test]
    fn a_transported_link_whose_interface_left_the_view_is_culled() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, true)).unwrap();
        let mut on_gone = entry(2, true);
        on_gone.next_hop_interface = iface(0xEE);
        transported.track(on_gone).unwrap();

        transported.cull_interface_orphans(|id| id != iface(0xEE));

        assert_eq!(transported.len(), 1);
        assert!(transported.entry_for(&LinkId::new([1; 16])).is_some());
        assert!(
            transported.entry_for(&LinkId::new([2; 16])).is_none(),
            "the row whose next hop left the view is gone",
        );
    }

    #[test]
    fn the_proof_gate_validates_once_over_the_right_side_only() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();

        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xA1),
                1,
                InstantMillis(2_000)
            ),
            Err(ValidateByProofError::WrongInterface),
            "a proof from the initiator's side validates nothing",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                2,
                InstantMillis(2_000)
            ),
            Err(ValidateByProofError::HopMismatch),
            "a hop mismatch validates nothing",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                1,
                InstantMillis(2_000)
            ),
            Ok(TransportSwitch {
                fire_on: iface(0xA1),
            }),
            "the right side and hop count validate and leave toward the initiator",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                1,
                InstantMillis(2_100)
            ),
            Err(ValidateByProofError::AlreadyValidated),
            "a validated row never re-validates",
        );
    }

    #[test]
    fn switching_obeys_direction_and_hops_and_refreshes_life() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, true)).unwrap();
        let link = LinkId::new([1; 16]);

        assert_eq!(
            transported.switch(&link, iface(0xA1), 1, InstantMillis(2_000)),
            Ok(TransportSwitch {
                fire_on: iface(0xB2),
            }),
            "a frame from the initiator's side leaves toward the destination",
        );
        assert_eq!(
            transported.switch(&link, iface(0xB2), 1, InstantMillis(2_100)),
            Ok(TransportSwitch {
                fire_on: iface(0xA1),
            }),
            "a frame from the destination's side leaves toward the initiator",
        );
        assert_eq!(
            transported.switch(&link, iface(0xA1), 7, InstantMillis(2_200)),
            Err(SwitchError::HopMismatch),
            "a hop mismatch repeats nothing",
        );
        assert_eq!(
            transported.switch(&link, iface(0xEE), 1, InstantMillis(2_300)),
            Err(SwitchError::WrongInterface),
            "an unknown interface repeats nothing",
        );

        assert_eq!(
            transported.earliest_deadline(),
            Some(InstantMillis(2_100 + TRANSPORTED_LINK_TIMEOUT_MS)),
            "every switched frame pushes the idle deadline",
        );
    }

    #[test]
    fn overdue_rows_drain_by_their_own_rule() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();
        transported.track(entry(2, true)).unwrap();

        assert_eq!(transported.pop_overdue(InstantMillis(8_999)), None);
        let popped = transported.pop_overdue(InstantMillis(9_000)).unwrap();
        assert_eq!(popped.link_id, LinkId::new([1; 16]));
        assert!(
            !popped.validated,
            "the unvalidated row dies at proof timeout"
        );

        assert_eq!(transported.pop_overdue(InstantMillis(9_000)), None);
        let popped = transported
            .pop_overdue(InstantMillis(1_000 + TRANSPORTED_LINK_TIMEOUT_MS))
            .unwrap();
        assert_eq!(popped.link_id, LinkId::new([2; 16]));
        assert!(transported.is_empty());
    }

    #[test]
    fn duplicates_and_overflow_are_refused() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();
        assert_eq!(transported.track(entry(1, false)), Err(ColumnsFull));
        transported.track(entry(2, false)).unwrap();
        transported.track(entry(3, false)).unwrap();
        assert_eq!(transported.track(entry(4, false)), Err(ColumnsFull));
    }

    #[test]
    fn the_extra_proof_allowance_is_one_mtu_of_airtime() {
        assert_eq!(extra_link_proof_timeout_ms(Some(500_000)), 8);
        assert_eq!(extra_link_proof_timeout_ms(Some(1_000)), 4_000);
        assert_eq!(extra_link_proof_timeout_ms(None), 0);
        assert_eq!(extra_link_proof_timeout_ms(Some(0)), 0);
    }
}

use heapless::Vec as HeaplessVec;

#[derive(Debug, Default)]
pub struct FixedTransportedLinkColumns<const MAX_TRANSIT_LINKS: usize> {
    entries: HeaplessVec<TransportedLink, MAX_TRANSIT_LINKS>,
}

impl<const MAX_TRANSIT_LINKS: usize> TransportedLinkColumns
    for FixedTransportedLinkColumns<MAX_TRANSIT_LINKS>
{
    fn capacity(&self) -> usize {
        MAX_TRANSIT_LINKS
    }
    fn len(&self) -> usize {
        self.entries.len()
    }
    fn entries(&self) -> &[TransportedLink] {
        &self.entries
    }
    fn entries_mut(&mut self) -> &mut [TransportedLink] {
        &mut self.entries
    }
    fn push(&mut self, entry: TransportedLink) -> Result<(), ColumnsFull> {
        self.entries.push(entry).map_err(|_| ColumnsFull)
    }
    fn swap_remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.swap_remove(index);
        }
    }
}

#[cfg(feature = "alloc")]
mod heap_transit_link_columns {
    use super::{ColumnsFull, TransportedLink, TransportedLinkColumns};
    use alloc::vec::Vec;

    pub const DEFAULT_MAX_TRANSPORTED_LINKS: usize = 1024;

    #[derive(Debug, Default)]
    pub struct HeapTransportedLinkColumns {
        entries: Vec<TransportedLink>,
    }

    impl TransportedLinkColumns for HeapTransportedLinkColumns {
        fn capacity(&self) -> usize {
            DEFAULT_MAX_TRANSPORTED_LINKS
        }
        fn len(&self) -> usize {
            self.entries.len()
        }
        fn entries(&self) -> &[TransportedLink] {
            &self.entries
        }
        fn entries_mut(&mut self) -> &mut [TransportedLink] {
            &mut self.entries
        }
        fn push(&mut self, entry: TransportedLink) -> Result<(), ColumnsFull> {
            if self.entries.len() >= DEFAULT_MAX_TRANSPORTED_LINKS {
                return Err(ColumnsFull);
            }
            self.entries.push(entry);
            Ok(())
        }
        fn swap_remove(&mut self, index: usize) {
            if index < self.entries.len() {
                self.entries.swap_remove(index);
            }
        }
    }
}

#[cfg(feature = "alloc")]
pub use heap_transit_link_columns::*;
