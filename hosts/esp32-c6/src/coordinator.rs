//! Spike A — synchronous two-interface coordinator on the C6.
//!
//! Forcing-function probe for the (frozen) host-runtime primitive: drive a
//! real USB Serial/JTAG interface *and* an in-process loopback through one
//! engine step, on a single core, with no async runtime — and see how
//! coordinating two interfaces *reads* in pure sync. This is the baseline we
//! measure an embassy version against before committing the primitive's
//! shape, so it stays deliberately concrete: no generic coordinator lands
//! here yet.
//!
//! The loopback is wired as a self-echoing wire — its second (unregistered)
//! half reflects each transmission back onto the registered half's inbound —
//! so it behaves like a live interface that hears its own echo rather than a
//! write-only sink. That gives the engine's announce dedup something real to
//! chew on, and exercises the full poll → step → dispatch → re-ingest loop.

use core::cell::RefCell;
use core::convert::Infallible;

use esp_hal::rng::Rng;
use esp_hal::time::Instant;
use esp_println::println;
use heapless::{Deque, Vec as HeaplessVec};

use personal_rns::engine::{
    step_engine, DefaultEngineState, InboundPacket, InstantMillis, RegisterInterfaceError,
};
use personal_rns::host::EngineHost;
use personal_rns::interfaces::{Interface, InterfaceId, NoAllocLoopback, NoAllocLoopbackQueue};
use personal_rns::wire::MTU;

use crate::usb_serial::Esp32UsbSerialInterface;

/// Loopback queue depth per direction: at most one rebroadcast in flight for
/// this probe, plus a slot of slack.
pub const LOOPBACK_QUEUE_CAP: usize = 2;

/// Inbound packets gathered per step: the one-shot boot seed plus one read
/// from each of the two interfaces.
const MAX_INBOUND_PER_STEP: usize = 3;

/// Caller-owned loopback queue. Both halves of the pair borrow a couple of
/// these for their whole lifetime, so the caller must hold them somewhere that
/// outlives the coordinator (the C6 `main` stack).
pub type LoopbackQueue = NoAllocLoopbackQueue<MTU, LOOPBACK_QUEUE_CAP>;

type Loopback<'q> = NoAllocLoopback<'q, MTU, LOOPBACK_QUEUE_CAP>;

/// Construct one empty loopback queue, keeping the heapless storage shape
/// behind [`LoopbackQueue`] out of `main`.
pub fn new_loopback_queue() -> LoopbackQueue {
    RefCell::new(Deque::new())
}

/// What one coordinated step observed — surfaced for on-device logging so we
/// can watch the seed fan out to both interfaces and the echo get deduped.
pub struct StepSummary {
    pub inbound_from_usb: bool,
    pub inbound_from_loopback: bool,
    pub seeded: bool,
    pub egress_dispatches: usize,
    pub accepted_announces: usize,
}

/// Owns the two interfaces and the loopback's echo-wire half, and drives one
/// engine step across them.
pub struct DualInterfaceCoordinator<'d, 'q> {
    usb: Esp32UsbSerialInterface<'d>,
    /// The loopback half registered with the engine as a routable interface.
    loopback: Loopback<'q>,
    /// The loopback's other half, deliberately unregistered. Each step we
    /// drain whatever the engine transmitted on `loopback` and write it
    /// straight back, so the registered half receives its own echo.
    echo_wire: Loopback<'q>,
    /// One-shot boot announce, injected from a synthetic *unregistered* source
    /// so its rebroadcast fans out to both real interfaces. `None` once
    /// consumed.
    seed: Option<&'static [u8]>,
    seed_source: InterfaceId,
}

impl<'d, 'q> DualInterfaceCoordinator<'d, 'q> {
    pub fn new(
        usb: Esp32UsbSerialInterface<'d>,
        loopback: Loopback<'q>,
        echo_wire: Loopback<'q>,
        seed: &'static [u8],
        seed_source: InterfaceId,
    ) -> Self {
        Self {
            usb,
            loopback,
            echo_wire,
            seed: Some(seed),
            seed_source,
        }
    }

    /// Register both real interfaces with the engine. The echo wire stays
    /// unregistered — it models the far end of the loopback, not a routable
    /// transport of its own.
    pub fn register(&self, state: &mut DefaultEngineState) -> Result<(), RegisterInterfaceError> {
        state.register_routable_interface(&self.usb)?;
        state.register_routable_interface(&self.loopback)?;
        Ok(())
    }

    /// One cooperative pass: reflect the echo wire, poll both interfaces into a
    /// borrowed batch, run a single engine step, dispatch its egress.
    pub fn step(&mut self, state: &mut DefaultEngineState, now: InstantMillis) -> StepSummary {
        self.reflect_echo_wire();

        let mut usb_buf = [0u8; MTU];
        let mut loop_buf = [0u8; MTU];
        let mut inbound: HeaplessVec<InboundPacket<'_>, MAX_INBOUND_PER_STEP> = HeaplessVec::new();

        let seeded = if let Some(seed) = self.seed.take() {
            let _ = inbound.push(InboundPacket {
                arrived_at: now,
                source_interface: self.seed_source,
                bytes: seed,
            });
            true
        } else {
            false
        };

        // `read_inbound` returns a packet borrowing the scratch buffer (not the
        // interface), so each `&mut` borrow is released before the host claims
        // the interfaces again for egress.
        let inbound_from_usb = match self.usb.read_inbound(&mut usb_buf, now) {
            Ok(Some(packet)) => {
                let _ = inbound.push(packet);
                true
            }
            Ok(None) => false,
            Err(e) => {
                println!("ESP32C6_USB_READ_ERR {e:?}");
                false
            }
        };

        let inbound_from_loopback = match self.loopback.read_inbound(&mut loop_buf, now) {
            Ok(Some(packet)) => {
                let _ = inbound.push(packet);
                true
            }
            Ok(None) => false,
            Err(e) => {
                println!("ESP32C6_LOOPBACK_READ_ERR {e:?}");
                false
            }
        };

        let mut host = CoordinatedStepHost {
            usb: &mut self.usb,
            loopback: &mut self.loopback,
            inbound: inbound.as_slice(),
            egress_dispatches: 0,
        };
        let out = step_engine(state, &mut host).expect("c6 host ops are infallible");
        let egress_dispatches = host.egress_dispatches;

        StepSummary {
            inbound_from_usb,
            inbound_from_loopback,
            seeded,
            egress_dispatches,
            accepted_announces: out.ingest.accepted_announce_count(),
        }
    }

    /// Drain everything the engine transmitted on the registered loopback half
    /// and write it straight back, so that half reads its own echo on the next
    /// poll. Read/write failures are logged and swallowed — a stuck echo wire
    /// must not wedge the coordinator.
    fn reflect_echo_wire(&mut self) {
        let mut echo_buf = [0u8; MTU];
        loop {
            match self.echo_wire.try_read(&mut echo_buf) {
                Ok(Some(n)) => {
                    if let Err(e) = self.echo_wire.write(&echo_buf[..n]) {
                        println!("ESP32C6_ECHO_REFLECT_ERR {e:?}");
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    println!("ESP32C6_ECHO_READ_ERR {e:?}");
                    break;
                }
            }
        }
    }
}

/// The per-step [`EngineHost`] for a coordinated pass: lends the borrowed
/// inbound batch and pumps egress to whichever interface the engine named.
struct CoordinatedStepHost<'a, 'd, 'q> {
    usb: &'a mut Esp32UsbSerialInterface<'d>,
    loopback: &'a mut Loopback<'q>,
    inbound: &'a [InboundPacket<'a>],
    egress_dispatches: usize,
}

impl EngineHost for CoordinatedStepHost<'_, '_, '_> {
    type Error = Infallible;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(
            Instant::now().duration_since_epoch().as_millis(),
        ))
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // TRNG-backed Rng (TrngSource is held alive in main); CSPRNG-grade per
        // the EngineHostView contract.
        Rng::new().read(buf);
        Ok(())
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(self.inbound)
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        // Pure tx pump: the engine already computed fire_on (registered
        // interfaces minus the source), so we just dispatch each id to its
        // interface. With two interfaces this is a legible if/else — and the
        // "find the interface for this id" shape is exactly what a registry
        // would own once the interface count grows. Surfacing that tension is
        // the point of the spike. Write failures are logged and swallowed per
        // the EngineHostView contract.
        for id in fire_on {
            if *id == self.usb.id() {
                self.egress_dispatches += 1;
                match self.usb.write(bytes) {
                    Ok(()) => println!("ESP32C6_DISPATCH iface=usb bytes={}", bytes.len()),
                    Err(e) => println!("ESP32C6_USB_EGRESS_ERR {e:?}"),
                }
            } else if *id == self.loopback.id() {
                self.egress_dispatches += 1;
                match self.loopback.write(bytes) {
                    Ok(()) => println!("ESP32C6_DISPATCH iface=loopback bytes={}", bytes.len()),
                    Err(e) => println!("ESP32C6_LOOPBACK_EGRESS_ERR {e:?}"),
                }
            } else {
                // The engine only names registered ids; a miss would mean
                // registry/dispatch drift, so surface it loudly.
                println!("ESP32C6_DISPATCH_UNKNOWN_INTERFACE");
            }
        }
        Ok(())
    }
}
