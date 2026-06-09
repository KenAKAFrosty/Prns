//! The embassy driver: the same reactor as [`tokio_reactor`](super::tokio_reactor), proven
//! against the no_std host. It races the same three inputs and runs the same engine
//! sink-methods through the same [`fire_due_lane`]/[`wait_for_due_lane`] the tokio driver
//! uses — only the channel and select primitives differ (`embassy_sync` + `embassy_futures`
//! for `tokio::sync` + `tokio::select!`). That the dispatch is shared and the loop body
//! reads the same is the point: the sync core's shape holds across std and no_std.

use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use embassy_time::{Duration, Timer};

use super::driver::{fire_due_lane, wait_for_due_lane};
use super::Host;
use crate::engine::{EngineReaction, EngineState, InstantMillis, IssuedCommand};
use crate::interfaces::substrate::EmbassyTimebase;
use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

/// A [`Host`] backed by embassy's clock and a caller-supplied entropy source. Mirrors the
/// legacy `EmbassyContractHost`: an [`EmbassyTimebase`] owns the clock and `draw_entropy`
/// is whatever the board hands it (an esp-hal `Rng`, a seeded test fill). The engine never
/// reads either — it asks the host.
pub struct EmbassyHost<E> {
    timebase: EmbassyTimebase,
    draw_entropy: E,
}

impl<E> EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    pub fn new(draw_entropy: E) -> Self {
        Self::new_with_timebase(EmbassyTimebase::capture_now(), draw_entropy)
    }

    pub fn new_with_timebase(timebase: EmbassyTimebase, draw_entropy: E) -> Self {
        Self {
            timebase,
            draw_entropy,
        }
    }
}

impl<E> Host for EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    fn now(&self) -> InstantMillis {
        self.timebase.now()
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        let remaining = deadline.0.saturating_sub(self.timebase.now().0);
        Timer::after(Duration::from_millis(remaining)).await;
    }

    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        (self.draw_entropy)(bytes);
    }
}

/// One inbound packet as it crosses the channel seam in no_std: the source interface and
/// the wire bytes in a fixed [`MTU`] buffer (no alloc, no zerocopy ring — the producer
/// fills one, the reactor borrows it mutably for in-place forwarding surgery). The std
/// driver carries `(InterfaceId, Vec<u8>)` instead; same pair, owned where it can be.
pub struct InboundFrame {
    pub source: InterfaceId,
    pub len: usize,
    pub bytes: [u8; MTU],
}

impl InboundFrame {
    #[must_use]
    pub fn new(source: InterfaceId, wire: &[u8]) -> Self {
        let len = wire.len().min(MTU);
        let mut bytes = [0u8; MTU];
        bytes[..len].copy_from_slice(&wire[..len]);
        Self { source, len, bytes }
    }
}

/// Run the reactor loop on an embassy executor until the task is dropped. Each turn parks
/// on the three inputs and runs the one sync engine method the winner names, pushing every
/// `EngineReaction` to `on_reaction`. `Idle` arms no timer, so the select rests on the two
/// channels and the core truly sleeps — the dormancy an MCU is built for.
pub async fn run<S, H, M, const INBOUND: usize, const COMMANDS: usize>(
    mut engine: EngineState<S>,
    view: &[InterfaceDescriptor],
    mut host: H,
    inbound: Receiver<'_, M, InboundFrame, INBOUND>,
    commands: Receiver<'_, M, IssuedCommand, COMMANDS>,
    mut on_reaction: impl FnMut(EngineReaction<'_>),
) where
    S: EngineStorage,
    H: Host,
    M: RawMutex,
{
    loop {
        let mut jitter_bytes = [0u8; core::mem::size_of::<u64>()];
        host.fill_entropy(&mut jitter_bytes);
        let jitter = JitterSeed(u64::from_le_bytes(jitter_bytes));
        let wake = engine.next_scheduled_wake(host.now());

        match select3(
            inbound.receive(),
            commands.receive(),
            wait_for_due_lane(&host, wake),
        )
        .await
        {
            Either3::First(mut frame) => {
                let now = host.now();
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: frame.source,
                    bytes: &mut frame.bytes[..frame.len],
                };
                engine.ingest_packet_into(
                    packet,
                    jitter,
                    view,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut on_reaction,
                );
            }
            Either3::Second(issued) => {
                let now = host.now();
                engine.ingest_command_into(
                    issued,
                    view,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut on_reaction,
                );
            }
            Either3::Third(lane) => {
                let now = host.now();
                fire_due_lane(&mut engine, lane, now, jitter, view, &mut on_reaction);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{hx, Cap, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
    use crate::engine::{Directive, Journaled};
    use crate::interfaces::{
        ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceMode,
        MediumKind, TransportCapability,
    };
    use crate::wire::{PacketType, WirePacketHeader};

    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_futures::yield_now;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::with_timeout;

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::vec::Vec;

    const WATCHDOG: Duration = Duration::from_secs(5);

    type SentLog = Rc<RefCell<Vec<(InterfaceId, Vec<u8>)>>>;

    fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            medium: MediumKind::Loopback,
            state: ConnectionState::Connected,
            announce_rate_limit: None,
        }
    }

    #[test]
    fn a_packet_wakes_the_embassy_reactor_to_rebroadcast_then_it_falls_dormant() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = [descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        let inbound: Channel<CriticalSectionRawMutex, InboundFrame, 2> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let sent: SentLog = Rc::new(RefCell::new(Vec::new()));
        let heard_sink = heard.clone();
        let sent_sink = sent.clone();

        let on_reaction = move |reaction: EngineReaction<'_>| match reaction {
            EngineReaction::Journaled(Journaled::AnnounceHeard { .. }) => {
                *heard_sink.borrow_mut() += 1;
            }
            EngineReaction::Journaled(
                Journaled::Delivered(_) | Journaled::CommandSettled { .. },
            ) => {}
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                sent_sink.borrow_mut().push((target, bytes.to_vec()));
            }
        };

        let raw = hx(RAW_ANNOUNCE);
        let original_hops = WirePacketHeader::parse(&raw)
            .expect("valid announce wire")
            .0
            .hops;

        let outcome = block_on(async {
            let reactor = run(
                engine,
                &view,
                EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
                inbound.receiver(),
                commands.receiver(),
                on_reaction,
            );

            let driver = async {
                // An idle reactor is silent: nothing heard, nothing sent.
                Timer::after(Duration::from_millis(50)).await;
                assert_eq!(*heard.borrow(), 0, "an idle reactor journals nothing");
                assert!(sent.borrow().is_empty(), "an idle reactor emits nothing");

                inbound.send(InboundFrame::new(source, &raw)).await;

                loop {
                    if *heard.borrow() >= 1 && !sent.borrow().is_empty() {
                        break;
                    }
                    yield_now().await;
                }
                sent.borrow().clone()
            };

            match select(reactor, with_timeout(WATCHDOG, driver)).await {
                Either::First(()) => unreachable!("the reactor loop never returns"),
                Either::Second(result) => {
                    result.expect("the rebroadcast fires before the watchdog")
                }
            }
        });

        assert_eq!(outcome.len(), 1, "exactly one rebroadcast directive");
        let (target, bytes) = &outcome[0];
        assert_eq!(
            *target, peer,
            "a rebroadcast fans to the peer, never back its source"
        );
        let (header, _) = WirePacketHeader::parse(bytes).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }
}
