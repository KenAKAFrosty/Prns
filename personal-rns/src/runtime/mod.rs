//! Runtime driver: the provided loop that drives the pure engine against a host.
//!
//! Defined in the core so every target body — daemon, microcontroller, SDK —
//! reuses one driver and supplies only a `HostAdapter`. Each `step` does both
//! engine verbs in order: ingest the host's queued batch, then advance one
//! periodic tick. The host decides how its queue fills and how often `step`
//! runs; the tick rate is the stable heartbeat the engine is designed against.

use crate::engine::{ingest, tick, EngineState, IngestOutput};
use crate::host::HostAdapter;
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};
use crate::wire::MTU;

/// Value-type summary of what a tick produced, captured before the
/// `TickOutput`'s lifetime ends so it can be returned in `StepOutput`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickSummary {
    pub egress_directive_count: usize,
    pub recovered_from_held_count: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub ingest: IngestOutput,
    pub tick: TickSummary,
}

/// One driver pass: pulls entropy, drains the inbound queue into
/// `ingest`, runs `tick`, then iterates every directive the tick
/// produced — serializing each via `EgressDirective::to_wire` and
/// handing the bytes (plus provenance) to the host's
/// `handle_egress`. The `TickOutput`'s Drop commits the tick's
/// emissions on the way out.
pub fn step<Host, R, A, H, D, const HELD: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
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
    let tick_out = tick(state, now, entropy);
    let tick_summary = TickSummary {
        egress_directive_count: tick_out.egress_directive_count(),
        recovered_from_held_count: tick_out.recovered_from_held_count(),
    };

    // Serialize each directive into a stack-sized MTU buffer and hand
    // the bytes + provenance to the host. The buffer fits any valid
    // announce (max-app_data no-ratchet announce is exactly MTU bytes).
    let mut emit_buf = [0u8; MTU];
    for directive in tick_out.egress_directives() {
        let n = directive
            .to_wire(&mut emit_buf)
            .expect("MTU-sized buf fits any valid wire packet");
        host.handle_egress(&emit_buf[..n], directive.received_from())?;
    }
    // tick_out drops here → state's due rebroadcasts are drained.

    Ok(StepOutput {
        ingest,
        tick: tick_summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DefaultEngineState;
    use crate::engine::{InboundPacket, InstantMillis};
    use crate::host::HostAdapter;
    use crate::interfaces::InterfaceId;
    use crate::routing::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::WirePacketHeader;

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

        fn handle_egress(
            &mut self,
            _bytes: &[u8],
            _received_from: Option<InterfaceId>,
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

        fn handle_egress(
            &mut self,
            _bytes: &[u8],
            _received_from: Option<InterfaceId>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn step_ticks_once_when_the_queue_is_empty() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut host = IdleHost;

        let out = step(&mut state, &mut host).unwrap();

        assert_eq!(state.tick_count(), 1);
        assert_eq!(out.ingest.processed_packet_count(), 0);
        assert_eq!(out.tick.egress_directive_count, 0);
    }

    #[test]
    fn step_ingests_the_whole_batch_then_ticks_once() {
        let queued = [
            InboundPacket {
                arrived_at: InstantMillis(5),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[0xAA],
            },
            InboundPacket {
                arrived_at: InstantMillis(6),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[0xBB, 0xCC],
            },
        ];
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut host = QueuedHost { queued: &queued };

        let out = step(&mut state, &mut host).unwrap();

        assert_eq!(out.ingest.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);
        assert_eq!(state.tick_count(), 1);
    }

    /// End-to-end host-facing contract: a single `step` ingests a real
    /// announce, accepts it, schedules a rebroadcast, and — once the
    /// host's `now` is past the jitter window — hands the wire bytes
    /// back to the host via `handle_egress` with the source-interface
    /// provenance threaded through.
    struct CapturingHost<'q> {
        queued: &'q [InboundPacket<'q>],
        now: InstantMillis,
        entropy: [u8; 8],
        handled: std::vec::Vec<(Option<InterfaceId>, std::vec::Vec<u8>)>,
    }

    impl HostAdapter for CapturingHost<'_> {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(self.now)
        }

        fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
            for (dst, src) in buf.iter_mut().zip(self.entropy.iter().cycle()) {
                *dst = *src;
            }
            Ok(())
        }

        fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
            Ok(self.queued)
        }

        fn handle_egress(
            &mut self,
            bytes: &[u8],
            received_from: Option<InterfaceId>,
        ) -> Result<(), Self::Error> {
            self.handled.push((received_from, bytes.to_vec()));
            Ok(())
        }
    }

    // A genuine RNS 1.3.1 announce (the same vector the engine module validates).
    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn step_hands_due_rebroadcasts_to_the_host_with_provenance_intact() {
        let raw = hx(RAW_ANNOUNCE);
        let source_interface = InterfaceId::new([0x7A; 16]);
        let arrival = InstantMillis(1_000);
        let queued = [InboundPacket {
            arrived_at: arrival,
            source_interface,
            bytes: &raw,
        }];
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut host = CapturingHost {
            queued: &queued,
            now: InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            entropy: 0xCAFE_F00D_DEAD_BEEFu64.to_le_bytes(),
            handled: std::vec::Vec::new(),
        };

        let out = step(&mut state, &mut host).unwrap();

        assert_eq!(out.ingest.accepted_announce_count(), 1);
        assert_eq!(out.ingest.scheduled_rebroadcast_count(), 1);
        assert_eq!(out.tick.egress_directive_count, 1);
        assert_eq!(host.handled.len(), 1);
        assert_eq!(host.handled[0].0, Some(source_interface));

        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let (rebroadcast_header, rebroadcast_payload) =
            WirePacketHeader::parse(&host.handled[0].1).unwrap();
        assert_eq!(rebroadcast_header.hops, original_header.hops + 1);
        assert_eq!(rebroadcast_header.destination, original_header.destination);
        assert_eq!(rebroadcast_payload, original_payload);
    }
}
