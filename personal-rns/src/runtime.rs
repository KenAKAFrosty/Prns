//! Runtime driver: the provided loop that drives the pure engine against a host.
//!
//! Defined in the core so every target body — daemon, microcontroller, SDK —
//! reuses one driver and supplies only a `Host`. Each `step` does both engine
//! verbs in order: ingest the host's queued batch, then advance one periodic
//! tick. The host decides how its queue fills and how often `step` runs; the
//! tick rate is the stable heartbeat the engine is designed against.

use crate::engine::{ingest, tick, EngineState, IngestOutput, TickOutput};
use crate::host::Host;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub ingest: IngestOutput,
    pub tick: TickOutput,
}

pub fn step<H: Host>(state: &mut EngineState, host: &mut H) -> Result<StepOutput, H::Error> {
    let packets = host.drain_packets()?;
    let ingest = ingest(state, packets);
    let now = host.now_millis()?;
    let tick = tick(state, now);
    Ok(StepOutput { ingest, tick })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineState, InboundPacket, InstantMillis};
    use crate::host::Host;

    /// Host with nothing queued — the steady idle case.
    #[derive(Default)]
    struct IdleHost;

    impl Host for IdleHost {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(InstantMillis(10))
        }

        fn drain_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
            Ok(&[])
        }

        fn transmit_packet(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Host that lends a fixed batch it borrows from the test.
    struct QueuedHost<'q> {
        queued: &'q [InboundPacket<'q>],
    }

    impl Host for QueuedHost<'_> {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(InstantMillis(10))
        }

        fn drain_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
            Ok(self.queued)
        }

        fn transmit_packet(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn step_ticks_once_when_the_queue_is_empty() {
        let mut state = EngineState::default();
        let mut host = IdleHost;

        let out = step(&mut state, &mut host).unwrap();

        assert_eq!(state.tick_count(), 1);
        assert_eq!(out.ingest.processed_packet_count(), 0);
        assert_eq!(out.tick.emitted_packet_count(), 0);
    }

    #[test]
    fn step_ingests_the_whole_batch_then_ticks_once() {
        let queued = [
            InboundPacket {
                arrival: InstantMillis(5),
                bytes: &[0xAA],
            },
            InboundPacket {
                arrival: InstantMillis(6),
                bytes: &[0xBB, 0xCC],
            },
        ];
        let mut state = EngineState::default();
        let mut host = QueuedHost { queued: &queued };

        let out = step(&mut state, &mut host).unwrap();

        assert_eq!(out.ingest.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);
        assert_eq!(state.tick_count(), 1);
    }
}
