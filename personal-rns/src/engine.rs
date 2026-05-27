//! Pure protocol engine boundary.
//!
//! The engine is driven by explicit state, inbound data, and caller-supplied
//! time. It does not read clocks, sockets, or storage directly.

/// Monotonic timestamp supplied by the host, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineState {
    tick_count: u64,
}

impl EngineState {
    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickInput<'a> {
    Idle { now: InstantMillis },
    InboundPacket { now: InstantMillis, bytes: &'a [u8] },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickOutput {
    emitted_packet_count: usize,
}

impl TickOutput {
    pub const fn emitted_packet_count(&self) -> usize {
        self.emitted_packet_count
    }
}

/// Advance the pure engine by one deterministic tick.
#[must_use]
pub fn tick(state: &mut EngineState, _input: TickInput<'_>) -> TickOutput {
    state.tick_count = state.tick_count.saturating_add(1);
    TickOutput::default()
}

#[cfg(test)]
mod tests {
    use super::{tick, EngineState, InstantMillis, TickInput, TickOutput};

    #[test]
    fn tick_is_deterministic_and_side_effect_free() {
        let mut left = EngineState::default();
        let mut right = EngineState::default();
        let input = TickInput::Idle {
            now: InstantMillis(1_000),
        };

        let left_output = tick(&mut left, input);
        let right_output = tick(&mut right, input);

        assert_eq!(left, right);
        assert_eq!(left_output, TickOutput::default());
        assert_eq!(right_output.emitted_packet_count(), 0);
    }
}
