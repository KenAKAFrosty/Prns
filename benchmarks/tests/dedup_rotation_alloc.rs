//! Rotation buffer-recycle gate. Once `HeapPacketHashHistory` has rotated twice,
//! both generation backings sit at the working-set high-water mark; every rotation
//! after that must reuse those buffers, allocating nothing. This guards the swap +
//! clear-retaining-capacity rotation against a regression to free + re-grow.

use personal_rns::routing::dedup::{
    HeapPacketHashHistory, PacketHash, PacketHashHistory, RememberPacketOutcome,
};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

struct Splitmix(u64);

impl Splitmix {
    fn next_hash(&mut self) -> PacketHash {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = self.0;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        PacketHash::new(bytes)
    }
}

fn fill_until_rotations(
    history: &mut HeapPacketHashHistory,
    entropy: &mut Splitmix,
    target: usize,
) {
    let mut rotations = 0;
    while rotations < target {
        if history.remember(entropy.next_hash()) == RememberPacketOutcome::StoredAfterRotation {
            rotations += 1;
        }
    }
}

#[test]
fn rotation_recycles_buffers_without_allocating() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let mut history = HeapPacketHashHistory::default();
    let capacity = history.generation_capacity();
    let mut entropy = Splitmix(0x1234_5678);

    fill_until_rotations(&mut history, &mut entropy, 2);

    let before = dhat::HeapStats::get();

    for _ in 0..=capacity {
        let outcome = history.remember(entropy.next_hash());
        assert_ne!(
            outcome,
            RememberPacketOutcome::AlreadyKnown,
            "distinct hashes must never read as duplicates across a rotation"
        );
    }

    let after = dhat::HeapStats::get();

    assert_eq!(
        after.total_blocks - before.total_blocks,
        0,
        "rotation must reuse buffers, but it allocated {} block(s) over {} steady-state inserts",
        after.total_blocks - before.total_blocks,
        capacity + 1,
    );
    assert_eq!(
        after.total_bytes - before.total_bytes,
        0,
        "rotation allocated {} bytes once both generation backings were established",
        after.total_bytes - before.total_bytes,
    );
}
