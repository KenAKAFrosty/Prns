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

/// Protocol state owned by the pure engine.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct State {
    ticks: u64,
}

impl State {
    /// Number of deterministic engine ticks applied to this state.
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }
}

/// Inbound input for one deterministic engine tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input<'a> {
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
pub fn tick(state: &mut State, _input: Input<'_>, _dt: DeltaMillis) -> Effects {
    state.ticks = state.ticks.saturating_add(1);
    Effects::default()
}

#[cfg(test)]
mod tests {
    use super::{tick, DeltaMillis, Effects, Input, InstantMillis, State};

    #[test]
    fn tick_is_deterministic_and_side_effect_free() {
        let mut left = State::default();
        let mut right = State::default();
        let input = Input::Idle {
            now: InstantMillis(1_000),
        };

        let left_effects = tick(&mut left, input, DeltaMillis(50));
        let right_effects = tick(&mut right, input, DeltaMillis(50));

        assert_eq!(left, right);
        assert_eq!(left_effects, Effects::default());
        assert_eq!(right_effects.emitted_packets(), 0);
    }
}
