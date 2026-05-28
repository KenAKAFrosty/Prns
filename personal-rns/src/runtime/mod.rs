//! Runtime driver: the provided loop that drives the pure engine against a host.
//!
//! Defined in the core so every target body — daemon, microcontroller, SDK —
//! reuses one driver and supplies only a `HostAdapter`. Each `step` does both engine
//! verbs in order: ingest the host's queued batch, then advance one periodic
//! tick. The host decides how its queue fills and how often `step` runs; the
//! tick rate is the stable heartbeat the engine is designed against.

use crate::engine::{ingest, tick, EngineState, IngestOutput, OutboundPacket, TickOutput};
use crate::host::HostAdapter;
use crate::outbox::Outbox;
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};
use heapless::Vec;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub ingest: IngestOutput,
    pub tick: TickOutput,
}

/// One driver pass: pulls entropy, drains the inbound queue into `ingest`,
/// runs `tick` against the supplied `outbox`, then surfaces whatever the tick
/// emitted to the host before clearing the outbox for the next pass.
///
/// `outbox` is lent by the host rather than owned by the engine: that puts
/// the byte/packet budget where MTU and batching policy are already known,
/// and keeps `EngineState` free of two more const generics.
pub fn step<Host, R, A, H, D, const HELD: usize, const ARENA: usize, const MAX_PACKETS: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
    outbox: &mut Outbox<ARENA, MAX_PACKETS>,
    host: &mut Host,
) -> Result<StepOutput, Host::Error>
where
    Host: HostAdapter,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    // Sip 8 bytes of host entropy for this step. CSPRNG-quality is the
    // host's contract (see `HostAdapter::fill_entropy`); the engine doesn't
    // check, it just consumes opaque bytes — same 8 bytes drive ingest jitter
    // and tick's held-recovery jitter so deterministic-replay tests can
    // compare bit-for-bit.
    let mut entropy_bytes = [0u8; 8];
    host.fill_entropy(&mut entropy_bytes)?;
    let entropy = u64::from_le_bytes(entropy_bytes);
    let packets = host.drain_inbound_packets()?;
    let ingest = ingest(state, packets, entropy);
    let now = host.now_millis()?;
    let tick = tick(state, now, entropy, outbox);
    // Materialize the outbox's transient borrow into a small stack vec so the
    // host sees a real slice. `MAX_PACKETS` matches the outbox cap, so push
    // can never fail.
    let mut emitted: Vec<OutboundPacket<'_>, MAX_PACKETS> = Vec::new();
    for packet in outbox.iter() {
        let _ = emitted.push(packet);
    }
    host.pump_outbound_packets(&emitted)?;
    drop(emitted);
    outbox.clear();
    Ok(StepOutput { ingest, tick })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DefaultEngineState;
    use crate::engine::{InboundPacket, InstantMillis, OutboundPacket};
    use crate::host::HostAdapter;

    /// Host with nothing queued — the steady idle case.
    #[derive(Default)]
    struct IdleHost;

    impl HostAdapter for IdleHost {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(InstantMillis(10))
        }

        fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
            // Deterministic zero-fill — non-CSPRNG, fine for the idle path.
            buf.fill(0);
            Ok(())
        }

        fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
            Ok(&[])
        }

        fn pump_outbound_packets(
            &mut self,
            _packets: &[OutboundPacket<'_>],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Host that lends a fixed batch it borrows from the test.
    struct QueuedHost<'q> {
        queued: &'q [InboundPacket<'q>],
    }

    impl HostAdapter for QueuedHost<'_> {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(InstantMillis(10))
        }

        fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
            buf.fill(0);
            Ok(())
        }

        fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
            Ok(self.queued)
        }

        fn pump_outbound_packets(
            &mut self,
            _packets: &[OutboundPacket<'_>],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn step_ticks_once_when_the_queue_is_empty() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut outbox = Outbox::<2048, 16>::new();
        let mut host = IdleHost;

        let out = step(&mut state, &mut outbox, &mut host).unwrap();

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
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut outbox = Outbox::<2048, 16>::new();
        let mut host = QueuedHost { queued: &queued };

        let out = step(&mut state, &mut outbox, &mut host).unwrap();

        assert_eq!(out.ingest.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);
        assert_eq!(state.tick_count(), 1);
    }
}
