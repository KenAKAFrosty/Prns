//! Pure protocol engine boundary.
//!
//! The engine has two verbs. `ingest` takes a batch of inbound packets, each
//! frozen with the instant it arrived, and is clock-free. `tick` advances the
//! engine's periodic work to a caller-supplied `now`. Neither reads clocks,
//! sockets, or storage directly.

/// Monotonic timestamp supplied by the host, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

/// One inbound packet, frozen with the instant it arrived. The host stamps
/// `arrival` when it enqueues the packet, so `ingest` processes a fixed record
/// and never needs to read a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrival: InstantMillis,
    pub bytes: &'a [u8],
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineState {
    tick_count: u64,
    ingested_packet_count: u64,
}

impl EngineState {
    /// Periodic `tick` passes run since construction.
    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// Inbound packets handed to `ingest` since construction.
    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
    }
}

/// What one `ingest` of an inbound batch produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestOutput {
    processed_packet_count: usize,
}

impl IngestOutput {
    pub const fn processed_packet_count(&self) -> usize {
        self.processed_packet_count
    }
}

/// What one periodic `tick` pass produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickOutput {
    emitted_packet_count: usize,
}

impl TickOutput {
    pub const fn emitted_packet_count(&self) -> usize {
        self.emitted_packet_count
    }
}

/// Process a batch of inbound packets. Clock-free: each packet carries its own
/// arrival instant, so the result is a pure function of `(state, packets)`. An
/// empty batch is valid and a no-op.
#[must_use]
pub fn ingest(state: &mut EngineState, packets: &[InboundPacket<'_>]) -> IngestOutput {
    state.ingested_packet_count = state
        .ingested_packet_count
        .saturating_add(packets.len() as u64);
    IngestOutput {
        processed_packet_count: packets.len(),
    }
}

/// Advance the engine's periodic work to `now`.
#[must_use]
pub fn tick(state: &mut EngineState, _now: InstantMillis) -> TickOutput {
    state.tick_count = state.tick_count.saturating_add(1);
    TickOutput::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_count_deterministically() {
        let mut left = EngineState::default();
        let mut right = EngineState::default();

        let left_out = tick(&mut left, InstantMillis(1_000));
        let right_out = tick(&mut right, InstantMillis(1_000));

        assert_eq!(left, right);
        assert_eq!(left.tick_count(), 1);
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.emitted_packet_count(), 0);
    }

    #[test]
    fn ingest_counts_the_batch_without_a_clock() {
        let mut state = EngineState::default();
        let batch = [
            InboundPacket {
                arrival: InstantMillis(10),
                bytes: &[1, 2, 3],
            },
            InboundPacket {
                arrival: InstantMillis(20),
                bytes: &[4],
            },
        ];

        let out = ingest(&mut state, &batch);
        assert_eq!(out.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);

        // Empty batch is valid and does not move state.
        let empty = ingest(&mut state, &[]);
        assert_eq!(empty.processed_packet_count(), 0);
        assert_eq!(state.ingested_packet_count(), 2);
    }
}
