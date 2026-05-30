//! The engine-driver seam: the per-step contract the pure engine is advanced
//! through, plus the default `step` that advances it one cycle.
//!
//! The engine is platform-agnostic and passive — off until something drives it.
//! An [`EngineDriver`] is a platform's supply of fuel for one cycle: the clock,
//! an entropy source, the inbound batch, and an egress sink. Its default
//! [`EngineDriver::step`] is one full engine pass — sip entropy, ingest the
//! inbound batch, tick, and emit each directive — so an implementor gets the
//! whole cycle for free and overrides only when it needs custom stepping.
//!
//! The over-time loop that fires `step` on a cadence, runs per-interface I/O,
//! and coordinates multiple drivers is the *runtime*, layered above this seam —
//! not part of it. The driver fuels the engine and advances it one cycle; the
//! runtime decides when.

use crate::engine::{ingest, tick, EngineState, InboundPacket, IngestOutput, InstantMillis};
use crate::interfaces::InterfaceId;
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

/// One platform's per-cycle fuel for the engine. A host implements the four
/// fuel methods; [`step`](EngineDriver::step) — one full engine pass — is
/// supplied by default.
pub trait EngineDriver {
    type Error;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error>;

    /// Fill `buf` with CSPRNG-quality random bytes. The engine consumes these
    /// as opaque data. RNS specifies the same bar
    /// (`os.urandom` or better) and we hold to it: a non-CSPRNG driver turns
    /// future crypto into latent forgeability bugs the engine cannot detect.
    ///
    /// Canonical implementations:
    /// - std platforms: `getrandom::getrandom(buf)` (the OS RNG shim)
    /// - ESP32-family: `esp_hal::rng::Rng` (hardware RNG peripheral)
    /// - Nordic nRF: `embassy_nrf::rng::Rng` (hardware RNG peripheral)
    ///
    /// Test drivers may seed deterministically (counter, fixed pattern) so
    /// determinism tests can compare byte-identical runs; tests are not
    /// crypto consumers and the engine doesn't enforce the contract at the
    /// trait surface.
    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Drain a batch of queued inbound packets, each stamped with its arrival
    /// instant. The driver owns the backing storage and lends the batch for one
    /// `ingest`. Draining need not be exhaustive: the driver may cap the batch
    /// so a burst can't make one [`step`](EngineDriver::step) do unbounded work
    /// — the remainder waits for the next call. An empty slice means nothing is
    /// queued.
    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error>;

    /// Handle one outgoing wire packet the engine produced.
    /// [`step`](EngineDriver::step) calls this once per directive the tick
    /// emitted, passing the already-serialized bytes and an engine-computed
    /// list of positive `fire_on` targets — exactly the interface ids the
    /// driver should transmit on. The list is never empty: the engine elides
    /// empty-fanout directives before they reach the driver.
    ///
    /// The driver is a pure tx/rx pump: write `bytes` to each id in
    /// `fire_on`. No "skip the source" logic is needed — the engine
    /// already filtered. As more directive variants land (data sends,
    /// proofs, link replies) the engine may compute different `fire_on`
    /// shapes per kind, but the driver's job stays the same: take the
    /// list, transmit on each.
    ///
    /// Returning `Err` propagates out of [`step`](EngineDriver::step) and halts
    /// the step early. Most drivers will want to log and swallow per-
    /// interface dispatch failures, returning `Ok` so the rest of the
    /// tick's directives still get a chance to dispatch.
    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error>;

    /// One engine pass: pulls entropy, drains the inbound queue into
    /// `ingest`, runs `tick`, then iterates every directive the tick
    /// produced — serializing each via `EgressDirective::to_wire` and
    /// handing the bytes (plus provenance) to this driver's
    /// `handle_egress`. The `TickOutput`'s Drop commits the tick's
    /// emissions on the way out.
    ///
    /// The engine state stays a passed argument — one driver can step several
    /// engines, and the engine's generic storage params live here on `step`
    /// rather than on the trait, keeping the trait itself free of them.
    /// Implementors get this for free; override only for custom stepping.
    fn step<R, A, H, D, const HELD: usize>(
        &mut self,
        state: &mut EngineState<R, A, H, D, HELD>,
    ) -> Result<StepOutput, Self::Error>
    where
        R: RouteColumns,
        A: RetainedAnnounceColumns,
        H: AnnounceIdHistory,
        D: RetainedAppData,
    {
        // Sip 8 bytes of driver entropy for this step. CSPRNG-quality is the
        // driver's contract (see `EngineDriver::fill_entropy`); the engine
        // doesn't check, it just consumes opaque bytes — same 8 bytes drive
        // ingest jitter and tick's held-recovery jitter so deterministic-replay
        // tests can compare bit-for-bit.
        let mut entropy_bytes = [0u8; 8];
        self.fill_entropy(&mut entropy_bytes)?;
        let entropy = u64::from_le_bytes(entropy_bytes);

        let packets = self.drain_inbound_packets()?;
        let ingest = ingest(state, packets, entropy);

        let now = self.now_millis()?;
        let tick_out = tick(state, now, entropy);
        let tick_summary = TickSummary {
            egress_directive_count: tick_out.egress_directive_count(),
            recovered_from_held_count: tick_out.recovered_from_held_count(),
        };

        // Serialize each directive into a stack-sized MTU buffer and hand
        // the bytes + engine-computed fanout targets to the driver. The
        // buffer fits any valid announce (max-app_data no-ratchet announce
        // is exactly MTU bytes).
        let mut emit_buf = [0u8; MTU];
        for directive in tick_out.egress_directives() {
            let n = directive
                .to_wire(&mut emit_buf)
                .expect("MTU-sized buf fits any valid wire packet");
            self.handle_egress(&emit_buf[..n], directive.fire_on())?;
        }
        // tick_out drops here → state's due rebroadcasts are drained.

        Ok(StepOutput {
            ingest,
            tick: tick_summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DefaultEngineState;
    use crate::engine::{InboundPacket, InstantMillis};
    use crate::interfaces::{
        Capabilities, ConnectionState, Interface, InterfaceId, InterfaceMode, MediumKind,
    };
    use crate::routing::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::WirePacketHeader;

    /// Driver with nothing queued — the steady idle case.
    #[derive(Default)]
    struct IdleEngineDriver;

    impl EngineDriver for IdleEngineDriver {
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
            _fire_on: &[InterfaceId],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Driver that lends a fixed batch it borrows from the test.
    struct QueuedEngineDriver<'q> {
        queued: &'q [InboundPacket<'q>],
    }

    impl EngineDriver for QueuedEngineDriver<'_> {
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
            _fire_on: &[InterfaceId],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn step_ticks_once_when_the_queue_is_empty() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let mut driver = IdleEngineDriver;

        let out = driver.step(&mut state).unwrap();

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
        let mut driver = QueuedEngineDriver { queued: &queued };

        let out = driver.step(&mut state).unwrap();

        assert_eq!(out.ingest.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);
        assert_eq!(state.tick_count(), 1);
    }

    /// End-to-end driver-facing contract: a single `step` ingests a real
    /// announce, accepts it, schedules a rebroadcast, and — once the
    /// driver's `now` is past the jitter window — hands the wire bytes
    /// back via `handle_egress` with an engine-computed positive `fire_on`
    /// list (registered interfaces minus the source).
    struct CapturingEngineDriver<'q> {
        queued: &'q [InboundPacket<'q>],
        now: InstantMillis,
        entropy: [u8; 8],
        handled: std::vec::Vec<(std::vec::Vec<InterfaceId>, std::vec::Vec<u8>)>,
    }

    impl EngineDriver for CapturingEngineDriver<'_> {
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
            fire_on: &[InterfaceId],
        ) -> Result<(), Self::Error> {
            self.handled.push((fire_on.to_vec(), bytes.to_vec()));
            Ok(())
        }
    }

    struct StaticInterface {
        id: InterfaceId,
    }

    impl StaticInterface {
        fn new(id: InterfaceId) -> Self {
            Self { id }
        }
    }

    impl Interface for StaticInterface {
        type Error = core::convert::Infallible;

        fn id(&self) -> InterfaceId {
            self.id
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                receives: true,
                transmits: true,
                forwards: true,
                repeats: false,
            }
        }

        fn mode(&self) -> InterfaceMode {
            InterfaceMode::Full
        }

        fn medium_kind(&self) -> MediumKind {
            MediumKind::Loopback
        }

        fn state(&self) -> ConnectionState {
            ConnectionState::Connected
        }

        fn try_read(&mut self, _buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
            Ok(None)
        }

        fn write(&mut self, _packet: &[u8]) -> Result<(), Self::Error> {
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
    fn step_hands_due_rebroadcasts_to_the_driver_with_engine_computed_fire_on() {
        let raw = hx(RAW_ANNOUNCE);
        let source_interface = InterfaceId::new([0x7A; 16]);
        let peer_interface = InterfaceId::new([0x7B; 16]);
        let arrival = InstantMillis(1_000);
        let queued = [InboundPacket {
            arrived_at: arrival,
            source_interface,
            bytes: &raw,
        }];
        let mut state: DefaultEngineState = DefaultEngineState::default();
        // Register both: the engine should compute fire_on = [peer]
        // (source excluded). Without `peer` registered, fanout would be
        // empty and the engine would elide the directive entirely.
        let source = StaticInterface::new(source_interface);
        let peer = StaticInterface::new(peer_interface);
        state.register_routable_interface(&source).unwrap();
        state.register_routable_interface(&peer).unwrap();
        let mut driver = CapturingEngineDriver {
            queued: &queued,
            now: InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            entropy: 0xCAFE_F00D_DEAD_BEEFu64.to_le_bytes(),
            handled: std::vec::Vec::new(),
        };

        let out = driver.step(&mut state).unwrap();

        assert_eq!(out.ingest.accepted_announce_count(), 1);
        assert_eq!(out.ingest.scheduled_rebroadcast_count(), 1);
        assert_eq!(out.tick.egress_directive_count, 1);
        assert_eq!(driver.handled.len(), 1);
        assert_eq!(
            driver.handled[0].0,
            std::vec![peer_interface],
            "engine must compute fire_on as registered interfaces minus the source"
        );

        let (original_header, original_payload) = WirePacketHeader::parse(&raw).unwrap();
        let (rebroadcast_header, rebroadcast_payload) =
            WirePacketHeader::parse(&driver.handled[0].1).unwrap();
        assert_eq!(rebroadcast_header.hops, original_header.hops + 1);
        assert_eq!(rebroadcast_header.destination, original_header.destination);
        assert_eq!(rebroadcast_payload, original_payload);
    }

    #[test]
    fn step_elides_directives_when_the_only_registered_interface_is_the_source() {
        // Edge case: the only target the engine knows about IS the source.
        // Fanout would be empty, so the engine elides the directive — the
        // driver sees no handle_egress call this tick. The rebroadcast is
        // still drained (engine processed it; nothing to do), so it won't
        // re-fire on a later tick.
        let raw = hx(RAW_ANNOUNCE);
        let source_interface = InterfaceId::new([0x7A; 16]);
        let arrival = InstantMillis(1_000);
        let queued = [InboundPacket {
            arrived_at: arrival,
            source_interface,
            bytes: &raw,
        }];
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let source = StaticInterface::new(source_interface);
        state.register_routable_interface(&source).unwrap();
        let mut driver = CapturingEngineDriver {
            queued: &queued,
            now: InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            entropy: 0xCAFE_F00D_DEAD_BEEFu64.to_le_bytes(),
            handled: std::vec::Vec::new(),
        };

        let out = driver.step(&mut state).unwrap();

        assert_eq!(out.ingest.accepted_announce_count(), 1);
        assert_eq!(out.tick.egress_directive_count, 0);
        assert!(driver.handled.is_empty());
        // The scheduled entry was elided AND drained; not still pending.
        assert_eq!(state.pending_rebroadcast_count(), 0);
    }
}
