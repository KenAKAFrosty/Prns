//! Substrate-neutral coordinator: owns the engine, runs one step against an
//! optional just-dequeued inbound frame, and stages egress for the harness to
//! write.
//!
//! Like [`crate::rns_frame_ingest`], this touches no hardware beyond the
//! always-available clock and TRNG (which read the same way on every substrate)
//! and contains no `.await`. A sync poll loop and an async coordinator task
//! call [`CoordinatorCore::step`] identically; the *only* substrate-specific
//! work is (a) how a frame is dequeued from the zero-copy channel before the
//! call and (b) how staged egress frames are written to the wire after it —
//! both of which are I/O, and so belong to the harness, not the core.
//!
//! Egress is **direct, not channelled**: the engine's synchronous
//! `handle_egress` stages frames into a small reused buffer, and the harness
//! flushes them straight to the owned TX with a non-blocking write. There is no
//! egress channel and no per-fanout owned message — staging is one reused
//! buffer, not a per-packet allocation.

use core::convert::Infallible;

use esp_hal::rng::Rng;
use esp_hal::time::Instant;
use heapless::Vec as HeaplessVec;

use personal_rns::engine::{DefaultEngineState, EngineDriver, InboundPacket, InstantMillis};
use personal_rns::interfaces::{
    Capabilities, ConnectionState, Interface, InterfaceId, InterfaceMode, MediumKind,
};

use crate::rns_frame_ingest::PacketBytes;

/// The single registered interface in this spike (USB). Byte pattern chosen for
/// log legibility, matching Spikes A and B.
pub const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xC6; 16]);

/// Synthetic, deliberately *unregistered* source for the boot seed, so its
/// rebroadcast fans out to USB rather than being excluded as the source.
const SEED_SOURCE_ID: InterfaceId = InterfaceId::new([0x7A; 16]);

/// Egress frames a single step can stage. One interface, bounded traffic — a
/// few slots of slack is plenty.
const MAX_EGRESS_PER_STEP: usize = 4;

/// Current time in milliseconds since boot — reads the same SystemTimer-derived
/// clock on every substrate.
pub fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

/// Egress frames produced by one step, awaiting the harness's wire write. A
/// reused buffer, cleared each step — not a per-message allocation.
pub struct EgressStaging {
    pub frames: HeaplessVec<PacketBytes, MAX_EGRESS_PER_STEP>,
}

impl EgressStaging {
    pub const fn new() -> Self {
        Self {
            frames: HeaplessVec::new(),
        }
    }
}

impl Default for EgressStaging {
    fn default() -> Self {
        Self::new()
    }
}

/// What one step observed, surfaced for on-device logging.
pub struct StepSummary {
    pub seeded: bool,
    pub inbound_from_usb: bool,
    pub egress: usize,
    pub accepted: usize,
}

/// Owns the engine and the one-shot boot seed. Substrate-agnostic.
pub struct CoordinatorCore {
    state: DefaultEngineState,
    seed: Option<&'static [u8]>,
}

impl CoordinatorCore {
    /// Build the engine and register the USB interface descriptor (the engine
    /// keeps only its id + capabilities; the peripheral lives in the harness).
    pub fn new(seed: &'static [u8]) -> Self {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        state
            .register_routable_interface(&UsbInterfaceDescriptor)
            .expect("usb descriptor is connected and transmits");
        Self {
            state,
            seed: Some(seed),
        }
    }

    pub fn registered_interfaces(&self) -> usize {
        self.state.registered_interfaces().len()
    }

    pub fn route_count(&self) -> usize {
        self.state.route_count()
    }

    pub fn tick_count(&self) -> u64 {
        self.state.tick_count()
    }

    pub fn ingested_packet_count(&self) -> u64 {
        self.state.ingested_packet_count()
    }

    /// One engine step. `inbound` is an optional USB frame the harness just
    /// dequeued from the zero-copy channel — borrowed straight from the channel
    /// slot, so the engine reads it with no further copy. Egress frames are
    /// staged into `egress` for the harness to write to the wire.
    pub fn step(
        &mut self,
        now: InstantMillis,
        inbound: Option<&[u8]>,
        egress: &mut EgressStaging,
    ) -> StepSummary {
        egress.frames.clear();

        let mut batch: HeaplessVec<InboundPacket<'_>, 2> = HeaplessVec::new();
        let seeded = if let Some(seed) = self.seed.take() {
            let _ = batch.push(InboundPacket {
                arrived_at: now,
                source_interface: SEED_SOURCE_ID,
                bytes: seed,
            });
            true
        } else {
            false
        };
        let inbound_from_usb = if let Some(bytes) = inbound {
            let _ = batch.push(InboundPacket {
                arrived_at: now,
                source_interface: USB_INTERFACE_ID,
                bytes,
            });
            true
        } else {
            false
        };

        let accepted = {
            let mut driver = StagingExampleEngineDriver {
                inbound: batch.as_slice(),
                egress,
            };
            let out = driver
                .step(&mut self.state)
                .expect("c6 driver ops are infallible");
            out.ingest.accepted_announce_count()
        };

        StepSummary {
            seeded,
            inbound_from_usb,
            egress: egress.frames.len(),
            accepted,
        }
    }
}

/// Inert stand-in registered so the engine knows the topology (id +
/// capabilities) for fanout. Its `try_read`/`write` are never reached: RX
/// arrives via the zero-copy channel and egress is written directly by the
/// harness, so the engine only ever consults this at registration.
struct UsbInterfaceDescriptor;

impl Interface for UsbInterfaceDescriptor {
    type Error = Infallible;

    fn id(&self) -> InterfaceId {
        USB_INTERFACE_ID
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
        InterfaceMode::PointToPoint
    }

    fn medium_kind(&self) -> MediumKind {
        MediumKind::DirectPeer
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

/// Per-step [`EngineDriver`]: lends the borrowed inbound batch and stages egress
/// frames bound for USB into the reused buffer. Clock and entropy read the
/// always-available esp-hal sources, so this driver is substrate-independent.
struct StagingExampleEngineDriver<'a> {
    inbound: &'a [InboundPacket<'a>],
    egress: &'a mut EgressStaging,
}

impl EngineDriver for StagingExampleEngineDriver<'_> {
    type Error = Infallible;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(now_millis())
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // TRNG-backed Rng (the TrngSource guard is held alive by the harness's
        // main); CSPRNG-grade per the EngineDriver contract.
        Rng::new().read(buf);
        Ok(())
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(self.inbound)
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        for id in fire_on {
            if *id == USB_INTERFACE_ID {
                let mut frame = PacketBytes::new();
                if frame.extend_from_slice(bytes).is_ok() {
                    let _ = self.egress.frames.push(frame);
                }
            }
        }
        Ok(())
    }
}
