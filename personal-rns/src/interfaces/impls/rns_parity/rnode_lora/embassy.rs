//! The embassy LoRa worker runs the RNode-compatible LoRa interface over
//! a `lora-phy` radio. The host builds the `LoRa<RK, DLY>` (the SX1262, its board
//! `InterfaceVariant`, and the front-end GPIOs) and hands it here, so this stays
//! HAL-agnostic: it names `lora-phy`, never an esp-hal or a specific board.
//!
//! Like the serial worker, the outbound queue lives here; the inbound mailbox it
//! stamps into belongs to the runtime (`runtime::channels::embassy`). LoRa is a
//! half-duplex broadcast medium,
//! so the loop sits in continuous RX and, when the runtime hands it a packet,
//! breaks off to transmit (one or two frames, RNode-split) and returns to RX —
//! never both at once. Received frames feed a [`LoRaReassembler`] that rebuilds
//! split packets before they reach the engine.

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};
use lora_phy::mod_params::{Bandwidth, CodingRate, RxMode, SpreadingFactor};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};

use super::core::{
    air_frame_count, encode_air_frame_part, LoRaModulation, LoRaReassembler, LORA_MAX_PAYLOAD,
    LORA_SINGLE_FRAME_MAX,
};
use crate::interfaces::{InboundSink, InterfaceWorkerContext};
use crate::runtime::channels::embassy_seam::EmbassyHostSubstrate;
use crate::wire::MTU;

/// SX1262 output power for the transmit ramp, in dBm. Low for a desk/bench gap:
/// the V4's external PA adds gain on top, and at near-field range the concern is
/// receiver saturation, not sensitivity ([[feedback_esp32c6_desk_tx_power]]).
/// Proven to reach a real RNode at ~33 cm; raise it for real range.
const TX_OUTPUT_POWER_DBM: i32 = -9;

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

/// What one pass of the contract-seam RX/TX select resolved to. `OutboundReady`
/// carries no packet: with the seam the transmit branch pulls it after the `rx`
/// borrow on the radio is released.
enum ServeStep {
    Received(usize),
    ReceiveFailed,
    OutboundReady,
}

/// Drive the LoRa link forever over the contract seam. A half-duplex loop: arm
/// continuous RX and wait for either a received frame (reassemble RNode's split →
/// `submit` the whole packet) or an outbound packet (frame it, splitting RNode-style
/// if needed → transmit, then return to RX).
///
/// Generic over the seam `DEPTH` and the lora-phy radio kind / delay. Inbound
/// packets are submitted through [`InboundSink::submit`]; outbound packets are
/// pulled with `ready` + `try_next_into`.
pub async fn serve<const DEPTH: usize, RK, DLY>(
    mut lora: LoRa<RK, DLY>,
    profile: LoRaModulation,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, DEPTH>>,
) where
    RK: RadioKind,
    DLY: DelayNs,
{
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
    // Scratch for one packet pulled off the outbound ring; async transmit needs
    // an owned packet buffer.
    let mut out_pkt = [0u8; MTU];
    let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
    // RNode's header sequence nibble; one value per packet (both frames of a split
    // share it), bumped after each send.
    let mut seq: u8 = 0;

    loop {
        if let Err(e) = lora
            .prepare_for_rx(RxMode::Continuous, &modulation, &rx_pkt)
            .await
        {
            log::warn!("RNS_LORA prepare_for_rx failed: {e:?}");
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        // Borrow the radio only for the wait; capture the outcome so the borrow is
        // released before the transmit branch reuses the radio.
        let step = {
            let rx_fut = lora.rx(&rx_pkt, &mut rx_buf);
            match select(rx_fut, context.outbound.ready()).await {
                Either::First(Ok((len, _status))) => ServeStep::Received(len as usize),
                Either::First(Err(e)) => {
                    log::warn!("RNS_LORA rx error: {e:?}");
                    ServeStep::ReceiveFailed
                }
                Either::Second(()) => ServeStep::OutboundReady,
            }
        };

        match step {
            ServeStep::ReceiveFailed => {}
            ServeStep::Received(len) => {
                // A whole packet (single frame, or the second part of a split) comes
                // back ready to submit.
                if let Some(packet) = reassembler.feed(&rx_buf[..len]) {
                    if !packet.is_empty() && packet.len() <= MTU {
                        if context
                            .inbound
                            .submit(|buf| {
                                buf[..packet.len()].copy_from_slice(packet);
                                packet.len()
                            })
                            .is_err()
                        {
                            log::warn!("RNS_LORA inbound ring full, dropped {}B", packet.len());
                        }
                    }
                }
            }
            ServeStep::OutboundReady => {
                let Some(plen) = context.outbound.try_next_into(&mut out_pkt) else {
                    continue;
                };
                // One frame, or two RNode-split frames sharing this packet's seq.
                for index in 0..air_frame_count(plen) {
                    match encode_air_frame_part(&out_pkt[..plen], seq, index, &mut tx_frame) {
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
