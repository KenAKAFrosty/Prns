//! Pure protocol engine boundary.
//!
//! The engine is driven by explicit state, inbound data, and caller-supplied
//! time. It does not read clocks, sockets, or storage directly.

/// Monotonic timestamp supplied by the host, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

/// Elapsed time supplied by the host for one engine tick, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeltaMillis(pub u64);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineState {
    ticks: u64,
}

impl EngineState {
    /// Number of deterministic engine ticks applied to this state.
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }
}

/// Inbound input for one deterministic engine tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickInput<'a> {
    Idle { now: InstantMillis },
    InboundPacket { now: InstantMillis, bytes: &'a [u8] },
}

/// Effects produced by one deterministic engine tick.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Effects {
    emitted_packets: usize,
}

impl Effects {
    /// Count of packets requested for host transmission.
    pub const fn emitted_packets(&self) -> usize {
        self.emitted_packets
    }
}

/// Advance the pure engine by one deterministic tick.
#[must_use]
pub fn tick(state: &mut EngineState, _input: TickInput<'_>, _dt: DeltaMillis) -> Effects {
    state.ticks = state.ticks.saturating_add(1);
    Effects::default()
}

#[cfg(test)]
mod tests {
    use super::{tick, DeltaMillis, Effects, TickInput, InstantMillis, EngineState};

    #[test]
    fn tick_is_deterministic_and_side_effect_free() {
        let mut left = EngineState::default();
        let mut right = EngineState::default();
        let input = TickInput::Idle {
            now: InstantMillis(1_000),
        };

        let left_effects = tick(&mut left, input, DeltaMillis(50));
        let right_effects = tick(&mut right, input, DeltaMillis(50));

        assert_eq!(left, right);
        assert_eq!(left_effects, Effects::default());
        assert_eq!(right_effects.emitted_packets(), 0);
    }
}
