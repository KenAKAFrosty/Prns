//! A fixed-capacity, alloc-free buffer for outbound packets the engine wants
//! transmitted.
//!
//! Variable-length packets packed into one byte arena behind a span table — the
//! same layout as `PackedAppDataArena`, but append-only: a producer reserves a slot
//! and writes a packet straight into the arena (no scratch or double copy), the
//! caller drains them with `iter()`, then `clear()` resets for the next round.
//!
//! The host *lends* an `Outbox` into the engine rather than the engine owning
//! one, so the byte/packet budget lives where the MTU and batch sizes are known,
//! and the engine state stays free of more const parameters. `iter()` hands out
//! packets that borrow the arena transiently, which is what lets a fixed buffer
//! return variable-length output at all — a stored `&[OutboundPacket]` into our
//! own arena would be a self-referential struct.
//!
//! `PartialEq` is structural — it compares the whole arena, including the dead
//! tail past `used` that `clear` leaves behind — so `==` means "identical
//! representation," not "same set of packets." Deliberate, and used only for
//! determinism tests, the same as `PackedAppDataArena`.

use crate::engine::OutboundPacket;
use heapless::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxFull {
    Bytes,
    Packets,
}

/// A packed, append-only buffer of outbound packets. `ARENA_BYTES` is the total
/// byte budget; `MAX_PACKET_COUNT` caps how many packets can be queued before a
/// drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbox<const ARENA_BYTES: usize, const MAX_PACKET_COUNT: usize> {
    arena: [u8; ARENA_BYTES],
    used: usize,
    spans: Vec<Span, MAX_PACKET_COUNT>,
}

impl<const ARENA_BYTES: usize, const MAX_PACKET_COUNT: usize> Default
    for Outbox<ARENA_BYTES, MAX_PACKET_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const ARENA_BYTES: usize, const MAX_PACKET_COUNT: usize>
    Outbox<ARENA_BYTES, MAX_PACKET_COUNT>
{
    pub const fn new() -> Self {
        Self {
            arena: [0u8; ARENA_BYTES],
            used: 0,
            spans: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Append one `len`-byte packet, built in place. `write` is handed an
    /// exactly-`len` slice of the arena and must fill it. Room is checked before
    /// `write` runs, so an `OutboxFull` leaves the buffer untouched.
    pub fn write_packet(
        &mut self,
        len: usize,
        write: impl FnOnce(&mut [u8]),
    ) -> Result<(), OutboxFull> {
        if len > ARENA_BYTES - self.used {
            return Err(OutboxFull::Bytes);
        }
        if self.spans.is_full() {
            return Err(OutboxFull::Packets);
        }

        let offset = self.used;
        write(&mut self.arena[offset..offset + len]);
        self.used += len;
        let _ = self.spans.push(Span { offset, len });
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = OutboundPacket<'_>> {
        self.spans.iter().map(|span| OutboundPacket {
            bytes: &self.arena[span.offset..span.offset + span.len],
        })
    }

    pub fn clear(&mut self) {
        self.used = 0;
        self.spans.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a fully-formed packet — the common case in tests.
    fn put<const B: usize, const P: usize>(
        outbox: &mut Outbox<B, P>,
        bytes: &[u8],
    ) -> Result<(), OutboxFull> {
        outbox.write_packet(bytes.len(), |buf| buf.copy_from_slice(bytes))
    }

    /// The buffer's core invariant: packets pack a contiguous prefix with no
    /// gaps, and `used` equals the sum of their lengths.
    fn assert_packed<const B: usize, const P: usize>(outbox: &Outbox<B, P>) {
        let mut expected_offset = 0;
        for span in &outbox.spans {
            assert_eq!(span.offset, expected_offset, "spans must be gap-free");
            expected_offset += span.len;
        }
        assert_eq!(expected_offset, outbox.used);
    }

    fn drained<const B: usize, const P: usize>(outbox: &Outbox<B, P>) -> std::vec::Vec<&[u8]> {
        outbox.iter().map(|p| p.bytes).collect()
    }

    #[test]
    fn push_then_iter_round_trips() {
        let mut outbox = Outbox::<64, 4>::new();
        put(&mut outbox, &[1, 2, 3]).unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(drained(&outbox), std::vec![&[1, 2, 3][..]]);
        assert_packed(&outbox);
    }

    #[test]
    fn write_packet_builds_a_packet_in_place_from_parts() {
        // The 3b shape: header bytes then payload, written straight into the slot.
        let mut outbox = Outbox::<64, 4>::new();
        let header = [0xDE, 0xAD];
        let payload = [0xBE, 0xEF, 0x00];
        outbox
            .write_packet(header.len() + payload.len(), |buf| {
                buf[..header.len()].copy_from_slice(&header);
                buf[header.len()..].copy_from_slice(&payload);
            })
            .unwrap();
        assert_eq!(
            drained(&outbox),
            std::vec![&[0xDE, 0xAD, 0xBE, 0xEF, 0x00][..]]
        );
        assert_packed(&outbox);
    }

    #[test]
    fn multiple_packets_iterate_in_append_order() {
        let mut outbox = Outbox::<64, 4>::new();
        put(&mut outbox, &[0xAA; 3]).unwrap();
        put(&mut outbox, &[0xBB; 5]).unwrap();
        put(&mut outbox, &[0xCC; 2]).unwrap();
        assert_eq!(
            drained(&outbox),
            std::vec![&[0xAA; 3][..], &[0xBB; 5][..], &[0xCC; 2][..]]
        );
        assert_packed(&outbox);
    }

    #[test]
    fn clear_empties_and_allows_reuse() {
        let mut outbox = Outbox::<64, 4>::new();
        put(&mut outbox, &[0xAA; 6]).unwrap();
        outbox.clear();
        assert!(outbox.is_empty());
        assert_eq!(outbox.iter().count(), 0);
        // Reusable after a drain: the next round starts from the top of the arena.
        put(&mut outbox, &[0xBB; 4]).unwrap();
        assert_eq!(drained(&outbox), std::vec![&[0xBB; 4][..]]);
        assert_packed(&outbox);
    }

    #[test]
    fn push_past_the_byte_budget_errors_and_leaves_the_outbox_unchanged() {
        let mut outbox = Outbox::<8, 4>::new();
        put(&mut outbox, &[0xAA; 6]).unwrap();
        let before = outbox.clone();
        assert_eq!(put(&mut outbox, &[0xBB; 4]), Err(OutboxFull::Bytes));
        assert_eq!(outbox, before); // error path mutated nothing
        assert_packed(&outbox);
    }

    #[test]
    fn push_past_the_packet_cap_errors() {
        let mut outbox = Outbox::<64, 2>::new();
        put(&mut outbox, &[1]).unwrap();
        put(&mut outbox, &[2]).unwrap();
        assert_eq!(put(&mut outbox, &[3]), Err(OutboxFull::Packets));
    }

    #[test]
    fn fills_the_byte_budget_exactly() {
        let mut outbox = Outbox::<8, 4>::new();
        put(&mut outbox, &[0x7; 8]).unwrap();
        assert_eq!(put(&mut outbox, &[0x9]), Err(OutboxFull::Bytes));
        assert_packed(&outbox);
    }

    #[test]
    fn identical_append_sequences_yield_byte_identical_outboxes() {
        fn build() -> Outbox<64, 4> {
            let mut o = Outbox::<64, 4>::new();
            put(&mut o, &[0xAA; 4]).unwrap();
            put(&mut o, &[0xBB; 7]).unwrap();
            o
        }
        assert_eq!(build(), build());
    }
}
