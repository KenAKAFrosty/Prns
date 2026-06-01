//! The embassy driver for a [`Manifold`] — the ESP32 family (S3, C6) and any
//! embassy-net host.
//!
//! This is the substrate-specific half of the runtime: it owns the clock
//! (embassy-time), the sleep primitive (`select` of inbound-ready vs the
//! engine's next deadline), and the shared inbound mailbox the workers stamp
//! into. The neutral aggregation point it drives is [`Manifold`].
//!
//! The mailbox types live here — with their draining end. The runtime drains
//! inbound, so [`InboxEntry`] and the inbound channel aliases belong to the
//! runtime; a worker is *handed* an [`InboundSender`] and stamps into it. (The
//! outbound queue drains the other way — into the worker — so it lives with the
//! worker shell.)

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::watch::{Receiver as WatchReceiver, Sender as WatchSender, Watch};
use embassy_time::{Instant as EmbassyInstant, Timer};
use heapless::Vec as HVec;

use super::super::{Manifold, RuntimeSnapshot};
use crate::engine::{EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledWakeup};
use crate::interfaces::{InterfaceId, InterfaceWorker};
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};

/// How many inbound packets the shared mailbox holds before a worker's stamp is
/// dropped. A drop self-heals — the engine re-emits announces on its own cadence
/// and Reticulum tolerates loss — so a shallow queue is the right trade on a
/// constrained node.
pub const INBOX_DEPTH: usize = 4;

/// One inbound packet a worker has stamped into the shared mailbox: the wire
/// bytes plus the provenance the manifold needs (which interface heard it, and
/// when). An *owned* [`InboundPacket`] — it must outlive the socket read that
/// produced it to ride into the next cycle's batch.
///
/// `PACKET_BUFFER_SIZE` is the producing worker's
/// [`InterfaceWorker::PACKET_BUFFER_SIZE`]; the host sizes the mailbox off that
/// one well-known number so a stamped packet always fits.
pub struct InboxEntry<const PACKET_BUFFER_SIZE: usize> {
    pub arrived_at: InstantMillis,
    pub source: InterfaceId,
    pub bytes: HVec<u8, PACKET_BUFFER_SIZE>,
}

/// The shared inbound mailbox: workers stamp [`InboxEntry`]s in, the runtime
/// drains them each cycle. Sized to the worker's `PACKET_BUFFER_SIZE`.
pub type InboundChannel<const PACKET_BUFFER_SIZE: usize> =
    Channel<CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;
/// The stamping end a worker holds.
pub type InboundSender<const PACKET_BUFFER_SIZE: usize> =
    Sender<'static, CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;
/// The draining end this driver holds.
pub type InboundReceiver<const PACKET_BUFFER_SIZE: usize> =
    Receiver<'static, CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;

/// How many subscribers can read the runtime snapshot. One display task today;
/// bump if more consumers (a metrics exporter, a control UI) subscribe.
pub const RUNTIME_SNAPSHOT_RECEIVERS: usize = 1;

/// The latest-wins channel the runtime fires its post-cycle [`RuntimeSnapshot`]
/// out on — `Watch`, not a queue, so a burst of cycles coalesces to the newest
/// value and a subscriber that wakes late never replays stale snapshots. An app
/// awaits [`changed`](WatchReceiver::changed) and so sleeps until engine state
/// actually moves — no polling.
pub type RuntimeSnapshotWatch =
    Watch<CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The publishing end the runtime loop holds.
pub type RuntimeSnapshotSender =
    WatchSender<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The subscribing end an app holds.
pub type RuntimeSnapshotReceiver =
    WatchReceiver<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

/// Drive `manifold` forever on the embassy substrate: aggregate the inbound the
/// workers stamped, cycle the engine, route egress, then sleep until either new
/// inbound arrives or the engine's next deadline. `draw_entropy` is the host's
/// CSPRNG (the one input the manifold can't supply itself); the clock is
/// embassy-time.
///
/// `PACKET_BUFFER_SIZE` is inferred from `inbound`, which the host sizes off the
/// registered worker's [`InterfaceWorker::PACKET_BUFFER_SIZE`] — the driver
/// never picks a size itself.
///
/// After each cycle it fires the manifold's [`RuntimeSnapshot`] out on
/// `snapshot_out` — the "read the data out into your app" seam. Sending every
/// cycle is cheap: cycles are sleepy (inbound or the next deadline), and the
/// `Watch` coalesces, so a subscriber sees the newest view and never a backlog.
pub async fn run<const PACKET_BUFFER_SIZE: usize, W, R, A, H, D, const MAX_HELD: usize, E>(
    mut manifold: Manifold<W, R, A, H, D, MAX_HELD>,
    inbound: InboundReceiver<PACKET_BUFFER_SIZE>,
    snapshot_out: RuntimeSnapshotSender,
    mut draw_entropy: E,
) where
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
    E: FnMut() -> EngineCycleEntropySeed,
{
    // A packet received while waiting carries into the next cycle's batch.
    let mut pending: Option<InboxEntry<PACKET_BUFFER_SIZE>> = None;
    loop {
        let mut msgs: HVec<InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH> = HVec::new();
        if let Some(msg) = pending.take() {
            let _ = msgs.push(msg);
        }
        while let Ok(msg) = inbound.try_receive() {
            if msgs.push(msg).is_err() {
                break;
            }
        }

        let mut batch: HVec<InboundPacket<'_>, INBOX_DEPTH> = HVec::new();
        for m in &msgs {
            let _ = batch.push(InboundPacket {
                arrived_at: m.arrived_at,
                source_interface: m.source,
                bytes: &m.bytes,
            });
        }

        let now = now_millis();
        let out = manifold.cycle_once(now, draw_entropy(), batch.iter().copied());
        if out.ingest.accepted_announce_count() > 0 {
            log::info!(
                "RNS_MANIFOLD RX accepted={} routes={}",
                out.ingest.accepted_announce_count(),
                manifold.engine().route_count(),
            );
        }
        drop(batch);
        drop(msgs);

        // Surface this cycle's state to whatever app is subscribed (e.g. the
        // host's display). Latest-wins, so this is fire-and-forget.
        snapshot_out.send(manifold.snapshot());

        let now = now_millis();
        match manifold.next_wakeup(now) {
            NextScheduledWakeup::Immediate => {}
            NextScheduledWakeup::At(deadline) => {
                let at = EmbassyInstant::from_millis(deadline.0);
                if let Either::First(msg) = select(inbound.receive(), Timer::at(at)).await {
                    pending = Some(msg);
                }
            }
            NextScheduledWakeup::Idle => {
                pending = Some(inbound.receive().await);
            }
        }
    }
}
