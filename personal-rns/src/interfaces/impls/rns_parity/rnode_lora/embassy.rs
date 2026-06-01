//! The embassy LoRa worker shell — runs the RNode-compatible LoRa interface over
//! a `lora-phy` radio. The host builds the `LoRa<RK, DLY>` (the SX1262, its board
//! `InterfaceVariant`, and the front-end GPIOs) and hands it here, so this stays
//! HAL-agnostic: it names `lora-phy`, never an esp-hal or a specific board.
//!
//! Like the serial shell, the outbound queue lives here (the shell drains it);
//! the inbound mailbox the shell *stamps into* belongs to the runtime
//! (`runtime::manifold::impls::embassy`). LoRa is a half-duplex broadcast medium,
//! so the loop sits in continuous RX and, when the runtime hands it a packet,
//! breaks off to transmit (one or two frames, RNode-split) and returns to RX —
//! never both at once. Received frames feed a [`LoRaReassembler`] that rebuilds
//! split packets before they reach the engine.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant as EmbassyInstant, Timer};
use heapless::Vec as HVec;
use lora_phy::mod_params::{Bandwidth, CodingRate, RxMode, SpreadingFactor};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};

use super::core::{
    air_frame_count, descriptor, encode_air_frame_part, LoRaModulation, LoRaReassembler,
    LORA_MAX_PAYLOAD, LORA_SINGLE_FRAME_MAX,
};
use crate::engine::{InstantMillis, OutboundPacket};
use crate::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, LinkState, QueueFull,
};
use crate::runtime::manifold::impls::embassy::{InboundSender, InboxEntry};
use crate::wire::MTU;

pub const OUTBOX_DEPTH: usize = 4;

/// SX1262 output power for the transmit ramp, in dBm. Low for a desk/bench gap:
/// the V4's external PA adds gain on top, and at near-field range the concern is
/// receiver saturation, not sensitivity ([[feedback_esp32c6_desk_tx_power]]).
/// Proven to reach a real RNode at ~33 cm; raise it for real range.
const TX_OUTPUT_POWER_DBM: i32 = -9;

/// Outbound packets are raw Reticulum wire packets (≤ engine [`MTU`]); the shell
/// frames each on the way out, splitting across two LoRa frames when it exceeds
/// what one carries (RNode-style).
pub type PacketBuf = HVec<u8, MTU>;

/// Outbound: packets the runtime hands the worker to transmit. The handle holds
/// the [`OutboundSender`]; the shell drains the [`OutboundReceiver`], frames each,
/// and transmits it. It lives here with its draining end — the same rule that
/// puts the inbound mailbox with the runtime that drains it.
pub type OutboundChannel = Channel<CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundSender = Sender<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundReceiver = Receiver<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;

/// A `'static` liveness flag shared between the handle and its shell: true while
/// the radio is up and listening. LoRa is a connectionless broadcast medium, so
/// there is no per-peer link state — `online` just means "the radio is operating".
pub type LinkUp = AtomicBool;

/// The worker handle for a LoRa interface on `id`. Cheap — a descriptor, the
/// outbound sender, and the shared liveness flag. The radio I/O runs in [`run`].
pub struct EmbassyRnodeLoraInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
    link_up: &'static LinkUp,
}

impl EmbassyRnodeLoraInterface {
    pub fn new(id: InterfaceId, outbound: OutboundSender, link_up: &'static LinkUp) -> Self {
        Self {
            descriptor: descriptor(id),
            outbound,
            link_up,
        }
    }
}

impl InterfaceWorker for EmbassyRnodeLoraInterface {
    // The engine MTU. A multi-worker host sizes its shared inbound mailbox to the
    // largest worker's buffer, so a reassembled LoRa packet always fits a mailbox
    // sized for this or anything larger.
    const PACKET_BUFFER_SIZE: usize = MTU;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn health(&self) -> InterfaceStats {
        InterfaceStats {
            link: LinkState::from_up(self.link_up.load(Ordering::Relaxed)),
            ..InterfaceStats::default()
        }
    }

    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull> {
        // `from_slice` rejects a packet larger than the engine MTU; the caller
        // treats that like a full queue and re-emits later.
        let buf = PacketBuf::from_slice(packet.bytes).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
    }
}

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

/// Map our HAL-agnostic [`LoRaModulation`] profile onto lora-phy's modulation
/// enums. `None` for a value lora-phy's SX126x backend can't express — our
/// shipped profiles are all expressible, so this only guards a future bad edit.
fn lora_phy_modulation(p: &LoRaModulation) -> Option<(SpreadingFactor, Bandwidth, CodingRate)> {
    let sf = match p.spreading_factor {
        5 => SpreadingFactor::_5,
        6 => SpreadingFactor::_6,
        7 => SpreadingFactor::_7,
        8 => SpreadingFactor::_8,
        9 => SpreadingFactor::_9,
        10 => SpreadingFactor::_10,
        11 => SpreadingFactor::_11,
        12 => SpreadingFactor::_12,
        _ => return None,
    };
    let bw = match p.bandwidth_hz {
        62_500 => Bandwidth::_62KHz,
        125_000 => Bandwidth::_125KHz,
        250_000 => Bandwidth::_250KHz,
        500_000 => Bandwidth::_500KHz,
        _ => return None,
    };
    let cr = match p.coding_rate_denominator {
        5 => CodingRate::_4_5,
        6 => CodingRate::_4_6,
        7 => CodingRate::_4_7,
        8 => CodingRate::_4_8,
        _ => return None,
    };
    Some((sf, bw, cr))
}

/// What one pass of the RX/TX select resolved to. Captured as an owned value so
/// the `&mut lora` borrow held by the in-flight `rx` future is released before we
/// act — the transmit branch needs the radio back.
enum Step {
    Received(usize),
    ReceiveFailed,
    Transmit(PacketBuf),
}

/// Drive the LoRa link forever. Build the modulation + packet params from
/// `profile`, then loop: arm continuous RX and wait for either a received frame
/// (reassemble RNode's split → stamp the whole packet into the shared mailbox) or
/// a packet from the worker queue (frame it, splitting across two LoRa frames if
/// needed → transmit, then return to RX).
///
/// Generic over `MAILBOX` (the host's `PACKET_BUFFER_SIZE`, ≥ this worker's) so
/// several worker kinds share one mailbox, and over the lora-phy radio kind /
/// delay so any SX126x board works. `id` stamps provenance on each inbound packet.
pub async fn run<const MAILBOX: usize, RK, DLY>(
    mut lora: LoRa<RK, DLY>,
    id: InterfaceId,
    profile: LoRaModulation,
    inbound: InboundSender<MAILBOX>,
    outbound: OutboundReceiver,
    link_up: &'static LinkUp,
) where
    RK: RadioKind,
    DLY: DelayNs,
{
    link_up.store(false, Ordering::Relaxed);

    let Some((sf, bw, cr)) = lora_phy_modulation(&profile) else {
        log::error!("RNS_LORA unsupported modulation profile; interface offline");
        return;
    };
    let modulation = match lora.create_modulation_params(sf, bw, cr, profile.frequency_hz) {
        Ok(m) => m,
        Err(e) => {
            log::error!("RNS_LORA modulation params failed: {e:?}");
            return;
        }
    };
    let rx_pkt = match lora.create_rx_packet_params(
        profile.preamble_symbols,
        false, // explicit header
        LORA_SINGLE_FRAME_MAX as u8,
        true,  // CRC on
        false, // IQ not inverted
        &modulation,
    ) {
        Ok(p) => p,
        Err(e) => {
            log::error!("RNS_LORA rx packet params failed: {e:?}");
            return;
        }
    };
    let mut tx_pkt = match lora.create_tx_packet_params(
        profile.preamble_symbols,
        false,
        true,
        false,
        &modulation,
    ) {
        Ok(p) => p,
        Err(e) => {
            log::error!("RNS_LORA tx packet params failed: {e:?}");
            return;
        }
    };

    let mut rx_buf = [0u8; LORA_SINGLE_FRAME_MAX];
    let mut tx_frame = [0u8; LORA_SINGLE_FRAME_MAX];
    let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
    // RNode's header sequence nibble; one value per *packet* (both frames of a
    // split share it so the peer reassembles them), bumped after each send.
    let mut seq: u8 = 0;

    loop {
        if let Err(e) = lora
            .prepare_for_rx(RxMode::Continuous, &modulation, &rx_pkt)
            .await
        {
            log::warn!("RNS_LORA prepare_for_rx failed: {e:?}");
            link_up.store(false, Ordering::Relaxed);
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }
        link_up.store(true, Ordering::Relaxed);

        // Borrow the radio only for the wait; capture the outcome so the borrow
        // is released before the transmit branch reuses the radio.
        let step = {
            let rx_fut = lora.rx(&rx_pkt, &mut rx_buf);
            match select(rx_fut, outbound.receive()).await {
                Either::First(Ok((len, _status))) => Step::Received(len as usize),
                Either::First(Err(e)) => {
                    log::warn!("RNS_LORA rx error: {e:?}");
                    Step::ReceiveFailed
                }
                Either::Second(packet) => Step::Transmit(packet),
            }
        };

        match step {
            Step::ReceiveFailed => {}
            Step::Received(len) => {
                // Feed the frame to the reassembler; a whole packet (single frame,
                // or the second part of a split) comes back ready to ingest.
                if let Some(packet) = reassembler.feed(&rx_buf[..len]) {
                    if !packet.is_empty() {
                        match HVec::<u8, MAILBOX>::from_slice(packet) {
                            Ok(bytes) => {
                                let entry = InboxEntry {
                                    arrived_at: now_millis(),
                                    source: id,
                                    bytes,
                                };
                                if inbound.try_send(entry).is_err() {
                                    log::warn!(
                                        "RNS_LORA inbound mailbox full, dropped {}B",
                                        packet.len()
                                    );
                                }
                            }
                            Err(_) => log::warn!(
                                "RNS_LORA reassembled packet {}B exceeds mailbox",
                                packet.len()
                            ),
                        }
                    }
                }
            }
            Step::Transmit(packet) => {
                // One frame, or two RNode-split frames sharing this packet's seq.
                for index in 0..air_frame_count(packet.len()) {
                    match encode_air_frame_part(&packet, seq, index, &mut tx_frame) {
                        Ok(n) => {
                            if let Err(e) = lora
                                .prepare_for_tx(
                                    &modulation,
                                    &mut tx_pkt,
                                    TX_OUTPUT_POWER_DBM,
                                    &tx_frame[..n],
                                )
                                .await
                            {
                                log::warn!("RNS_LORA prepare_for_tx failed: {e:?}");
                                break;
                            }
                            if let Err(e) = lora.tx().await {
                                log::warn!("RNS_LORA tx failed: {e:?}");
                                break;
                            }
                        }
                        Err(e) => {
                            log::warn!("RNS_LORA frame {index} encode failed: {e:?}");
                            break;
                        }
                    }
                }
                seq = seq.wrapping_add(0x10);
            }
        }
    }
}
