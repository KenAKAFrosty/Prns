use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant as EmbassyInstant, Timer};

use super::core::{decode_frame, EspNowFrameWriter, ESP_NOW_MAX_FRAME_PAYLOAD};
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{InboundSink, InterfaceWorkerContext};
use crate::wire::MTU;

/// How long to keep packing a frame after its first packet before transmitting.
/// Coalescing trades this much latency for far fewer transmissions when the
/// engine emits a burst. One millisecond is short next to a frame's airtime and
/// long enough to catch a same-cycle burst.
const COALESCE_LINGER: Duration = Duration::from_millis(1);

/// Both methods are `async` and not
/// `Send`-bounded — the worker runs on the host's single embassy executor,
/// joined with the other workers, never sent across threads.
#[allow(async_fn_in_trait)]
pub trait EspNowLink {
    type Error: core::fmt::Debug;

    async fn broadcast(&mut self, frame: &[u8]) -> Result<(), Self::Error>;

    async fn receive_into(&mut self, buf: &mut [u8]) -> usize;
}

fn submit_frame_packets(frame: &[u8], inbound: &mut impl InboundSink) {
    match decode_frame(frame) {
        Ok(reader) => {
            let mut stamped = 0usize;
            for packet in reader {
                if packet.is_empty() || packet.len() > MTU {
                    continue;
                }
                match inbound.submit(|buf| {
                    buf[..packet.len()].copy_from_slice(packet);
                    packet.len()
                }) {
                    Ok(()) => stamped += 1,
                    Err(_) => {
                        log::warn!("RNS_ESPNOW inbound ring full, dropped {}B", packet.len())
                    }
                }
            }
            if stamped > 0 {
                log::info!(
                    "RNS_ESPNOW rx frame: {stamped} packet(s) in {}B",
                    frame.len()
                );
            }
        }
        Err(e) => log::warn!("RNS_ESPNOW dropping malformed frame: {e:?}"),
    }
}

pub async fn serve<const MAX_BUFFERED_PACKETS: usize, L>(
    mut link: L,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
) where
    L: EspNowLink,
{
    let mut rx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut tx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];

    loop {
        if context.outbound.lease().is_none() {
            match select(link.receive_into(&mut rx_buf), context.outbound.ready()).await {
                Either::First(len) => {
                    submit_frame_packets(&rx_buf[..len], &mut context.inbound);
                    continue;
                }
                Either::Second(()) => {}
            }
        }
        let Some(mut lease) = context.outbound.lease() else {
            continue;
        };

        let mut writer = EspNowFrameWriter::new(&mut tx_buf);
        if !writer.try_push(lease.packet()) {
            log::warn!(
                "RNS_ESPNOW packet {}B too large for one frame, dropped",
                lease.packet().len()
            );
            lease.complete();
            continue;
        }
        lease.complete();

        let deadline = EmbassyInstant::now() + COALESCE_LINGER;
        loop {
            if let Some(mut lease) = context.outbound.lease() {
                if writer.try_push(lease.packet()) {
                    lease.complete();
                    continue;
                }
                // Doesn't fit this frame; kept, it opens the next frame.
                lease.keep();
                break;
            }
            match select(Timer::at(deadline), context.outbound.ready()).await {
                Either::First(_) => break,
                Either::Second(()) => {}
            }
        }

        let packed = writer.packet_count();
        if let Err(e) = link.broadcast(writer.frame()).await {
            log::warn!("RNS_ESPNOW broadcast of {packed} packet(s) failed: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::substrate::{
        new_wake_signal, EmbassyInterfaceChannels, EmbassyInterfaceSeam, WakeSignal,
    };
    use crate::interfaces::{InterfaceHandle, InterfaceId};

    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_futures::yield_now;
    use embassy_time::with_timeout;

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::vec::Vec;

    const WATCHDOG: Duration = Duration::from_secs(5);

    struct RecordingLink {
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
    }

    impl EspNowLink for RecordingLink {
        type Error = core::convert::Infallible;

        async fn broadcast(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
            self.sent.borrow_mut().push(frame.to_vec());
            Ok(())
        }

        async fn receive_into(&mut self, _buf: &mut [u8]) -> usize {
            core::future::pending().await
        }
    }

    fn id() -> InterfaceId {
        InterfaceId::new([0x33; 16])
    }

    fn transmitted_packets(frames: &[Vec<u8>]) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        for frame in frames {
            for packet in decode_frame(frame).expect("worker emits well-formed frames") {
                packets.push(packet.to_vec());
            }
        }
        packets
    }

    /// Run the real worker loop against a recording link until `expected`
    /// packets hit the wire (then settle, so over-transmission is also seen),
    /// or fail loudly on the watchdog. A worker that forgets to complete a
    /// lease either re-transmits (caught by the exact-count assertion) or
    /// spins without draining (caught by the watchdog).
    fn run_worker_until_drained<const MAX_BUFFERED_PACKETS: usize>(
        worker_context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
        expected: usize,
    ) -> Vec<Vec<u8>> {
        let sent: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        let link = RecordingLink { sent: sent.clone() };

        let outcome = block_on(async {
            let drained = async {
                loop {
                    if transmitted_packets(&sent.borrow()).len() >= expected {
                        break;
                    }
                    yield_now().await;
                }
                Timer::after(Duration::from_millis(20)).await;
            };
            match select(serve(link, worker_context), with_timeout(WATCHDOG, drained)).await {
                Either::First(()) => unreachable!("serve never returns"),
                Either::Second(result) => result,
            }
        });

        assert!(
            outcome.is_ok(),
            "worker failed to put {expected} packet(s) on the wire before the watchdog",
        );
        let frames = sent.borrow().clone();
        transmitted_packets(&frames)
    }

    #[test]
    fn queued_packets_reach_the_wire_exactly_once_and_in_order() {
        static CH: EmbassyInterfaceChannels<MTU, 8> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        for byte in [0xA1u8, 0xB2, 0xC3] {
            runtime_handle
                .acquire_send_grant(|buf| {
                    buf[..4].fill(byte);
                    4
                })
                .unwrap();
        }

        let packets = run_worker_until_drained(worker_context, 3);
        assert_eq!(
            packets,
            std::vec![std::vec![0xA1; 4], std::vec![0xB2; 4], std::vec![0xC3; 4]],
        );
    }

    #[test]
    fn packets_overflowing_one_frame_are_kept_and_open_the_next_frame() {
        static CH: EmbassyInterfaceChannels<MTU, 8> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        // Three MTU-sized packets cannot share one 1470-byte frame, so the
        // worker must carry the third over via a kept lease.
        for byte in [0x11u8, 0x22, 0x33] {
            runtime_handle
                .acquire_send_grant(|buf| {
                    buf.fill(byte);
                    MTU
                })
                .unwrap();
        }

        let packets = run_worker_until_drained(worker_context, 3);
        assert_eq!(
            packets,
            std::vec![
                std::vec![0x11; MTU],
                std::vec![0x22; MTU],
                std::vec![0x33; MTU]
            ],
        );
    }
}
