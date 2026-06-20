//! The embassy SX1262 worker: owns the radio over `lora-phy`, bridges its RX/TX to the reactor
//! seam, and reconfigures the radio in place when the app signals a new profile. Generic over the
//! `lora-phy` `RadioKind`, so the same body drives a Heltec V4 (esp-hal SPI) or an nRF SX1262 board
//! and compile-checks on the host. A reconfigure retunes the silicon and, when it changes the
//! channel identity, emits a `Retag` so the reactor re-keys the interface in place.

use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::DynamicSender;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::Vec as HeaplessVec;
use lora_phy::mod_params::{
    Bandwidth as PhyBandwidth, CodingRate as PhyCodingRate, ModulationParams, PacketParams, RxMode,
    SpreadingFactor as PhySpreadingFactor,
};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};

use crate::interfaces::rns_parity::rnode_lora::core::{
    self, air_frame_count, encode_air_frame_part, CodingRate, LoRaReassembler, LoraBandwidth,
    Modulation, RadioProfile, SpreadingFactor, LORA_MAX_PAYLOAD, LORA_SINGLE_FRAME_MAX,
    CHANNEL_TAG_CAP,
};
use crate::engine::InstantMillis;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::duty_gate::{DutyGate, DutyVerdict, FixedDutyQueue};
use crate::reactor::impls::embassy_reactor::{EmbassyInterfaceStatus, InterfaceLifecycle};
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// How often a serving radio re-checks its enabled gate, so a "Power" toggle from the UI takes
/// effect within a beat rather than waiting on traffic.
pub const ENABLED_POLL: Duration = Duration::from_millis(250);
/// The pause after a failed `prepare_for_rx` before retrying, so a transient radio fault doesn't spin.
const RX_RETRY_WAIT: Duration = Duration::from_millis(500);
/// How many held packets the duty gate buffers before dropping the oldest. The region's airtime
/// budget caps the queue sooner on a slow link, so this only bounds the buffer's memory.
const DUTY_QUEUE_FRAMES: usize = 4;

/// The signal the app holds to reconfigure a running radio: it sends a whole new [`RadioProfile`]
/// and the worker rebuilds the silicon's params from it. The granular, menu-shaped control surface
/// (set-frequency, set-modulation, …) lands with the reconfigure arc.
pub type LoRaControl = Signal<CriticalSectionRawMutex, RadioProfile>;

/// The `lora-phy` params one profile resolves to — rebuilt whole on a reconfigure.
struct RadioParams {
    modulation: ModulationParams,
    rx_pkt: PacketParams,
    tx_pkt: PacketParams,
}

/// Map a profile's LoRa modulation onto `lora-phy`'s enums. `None` when the profile is GFSK —
/// lora-phy 3.0.1 drives only the LoRa packet engine, so the speed mode waits on its own path.
fn phy_lora_params(
    profile: &RadioProfile,
) -> Option<(PhySpreadingFactor, PhyBandwidth, PhyCodingRate, u32)> {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation
    else {
        return None;
    };
    let sf = match spreading_factor {
        SpreadingFactor::Sf5 => PhySpreadingFactor::_5,
        SpreadingFactor::Sf6 => PhySpreadingFactor::_6,
        SpreadingFactor::Sf7 => PhySpreadingFactor::_7,
        SpreadingFactor::Sf8 => PhySpreadingFactor::_8,
        SpreadingFactor::Sf9 => PhySpreadingFactor::_9,
        SpreadingFactor::Sf10 => PhySpreadingFactor::_10,
        SpreadingFactor::Sf11 => PhySpreadingFactor::_11,
        SpreadingFactor::Sf12 => PhySpreadingFactor::_12,
    };
    let bw = match bandwidth {
        LoraBandwidth::Bw125kHz => PhyBandwidth::_125KHz,
        LoraBandwidth::Bw250kHz => PhyBandwidth::_250KHz,
        LoraBandwidth::Bw500kHz => PhyBandwidth::_500KHz,
    };
    let cr = match coding_rate {
        CodingRate::Cr45 => PhyCodingRate::_4_5,
        CodingRate::Cr46 => PhyCodingRate::_4_6,
        CodingRate::Cr47 => PhyCodingRate::_4_7,
        CodingRate::Cr48 => PhyCodingRate::_4_8,
    };
    Some((sf, bw, cr, profile.frequency.hz()))
}

fn build_params<RK: RadioKind, DLY: DelayNs>(
    radio: &mut LoRa<RK, DLY>,
    profile: &RadioProfile,
) -> Option<RadioParams> {
    let (sf, bw, cr, frequency_hz) = phy_lora_params(profile)?;
    let modulation = radio
        .create_modulation_params(sf, bw, cr, frequency_hz)
        .ok()?;
    let rx_pkt = radio
        .create_rx_packet_params(
            profile.preamble.count(),
            false,
            LORA_SINGLE_FRAME_MAX as u8,
            true,
            false,
            &modulation,
        )
        .ok()?;
    let tx_pkt = radio
        .create_tx_packet_params(profile.preamble.count(), false, true, false, &modulation)
        .ok()?;
    Some(RadioParams {
        modulation,
        rx_pkt,
        tx_pkt,
    })
}

/// The [`Retag`](InterfaceLifecycle::Retag) a reconfigure to `new_profile` warrants, or `None` when
/// the change leaves the channel identity untouched — a local knob like transmit power or preamble.
/// The channel_tag (frequency + modulation) is what mints the id, so only a change to it re-keys.
fn retag_message(current_id: InterfaceId, new_profile: &RadioProfile) -> Option<InterfaceLifecycle> {
    let new_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(new_profile));
    (new_id != current_id).then(|| InterfaceLifecycle::Retag {
        old_id: current_id,
        new_id,
        config: core::descriptor(new_id, new_profile),
    })
}

/// The summed on-air airtime of a packet's frames (1 or 2) — the currency the duty gate budgets in.
fn packet_airtime(packet: &[u8], bitrate_bps: u32) -> u64 {
    let mut scratch = [0u8; LORA_SINGLE_FRAME_MAX];
    let mut total = 0;
    for index in 0..air_frame_count(packet.len()) {
        if let Ok(n) = encode_air_frame_part(packet, 0, index, &mut scratch) {
            total += frame_airtime_us(n, bitrate_bps);
        }
    }
    total
}

/// Split a packet into its RNode frames and transmit each, recording airtime and throughput. Shared
/// by the immediate path and the duty gate's release, so a held packet rides out byte-for-byte the
/// same as an unheld one.
#[allow(clippy::too_many_arguments)]
async fn transmit_packet<RK: RadioKind, DLY: DelayNs>(
    radio: &mut LoRa<RK, DLY>,
    params: &mut RadioParams,
    power_dbm: i32,
    packet: &[u8],
    seq: &mut u8,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    status: &EmbassyInterfaceStatus,
    bitrate_bps: u32,
    now: InstantMillis,
    tx_frame: &mut [u8; LORA_SINGLE_FRAME_MAX],
) {
    for index in 0..air_frame_count(packet.len()) {
        let n = match encode_air_frame_part(packet, *seq, index, tx_frame) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("RNS_LORA frame {index} encode failed: {e:?}");
                break;
            }
        };
        if let Err(e) = radio
            .prepare_for_tx(&params.modulation, &mut params.tx_pkt, power_dbm, &tx_frame[..n])
            .await
        {
            log::warn!("RNS_LORA prepare_for_tx failed: {e:?}");
            break;
        }
        if let Err(e) = radio.tx().await {
            log::warn!("RNS_LORA tx failed: {e:?}");
            break;
        }
        status.add_tx(n as u64);
        throughput.record_tx(now, n as u64);
        status.set_transfer_rates(throughput.rates());
        status.set_airtime(airtime.record_tx(now, frame_airtime_us(n, bitrate_bps)));
    }
    *seq = seq.wrapping_add(0x10);
}

/// One SX1262 spoken as an RNode-compatible LoRa interface. Owns the radio for its whole life; the
/// `control` signal lets the app retune it live, the `status` handle carries its enable gate and
/// counters. `tag` is the channel identity the id derived from, recomputed only on a re-key.
pub struct LoRaInterface<'a, RK: RadioKind, DLY: DelayNs> {
    id: InterfaceId,
    radio: LoRa<RK, DLY>,
    profile: RadioProfile,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    control: &'a LoRaControl,
    status: &'a EmbassyInterfaceStatus,
    retag: DynamicSender<'a, InterfaceLifecycle>,
}

impl<'a, RK: RadioKind, DLY: DelayNs> LoRaInterface<'a, RK, DLY> {
    /// The id a radio on `profile` will carry — for the caller that stands its
    /// [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(profile))
    }

    #[must_use]
    pub fn new(
        radio: LoRa<RK, DLY>,
        profile: RadioProfile,
        control: &'a LoRaControl,
        status: &'a EmbassyInterfaceStatus,
        retag: DynamicSender<'a, InterfaceLifecycle>,
    ) -> Self {
        let tag = core::channel_tag(&profile);
        let id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &tag);
        Self {
            id,
            radio,
            profile,
            tag,
            control,
            status,
            retag,
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }
}

impl<RK: RadioKind, DLY: DelayNs> Interface for LoRaInterface<'_, RK, DLY> {
    const HW_MTU: usize = LORA_MAX_PAYLOAD;
    const KIND: InterfaceKind = InterfaceKind::LoRa;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, &self.profile)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let LoRaInterface {
            id,
            mut radio,
            mut profile,
            tag: _,
            control,
            status,
            retag,
        } = self;
        let mut current_id = id;

        let Some(mut params) = build_params(&mut radio, &profile) else {
            log::error!("RNS_LORA unsupported modulation profile; interface offline");
            status.set_connection(ConnectionState::Disconnected);
            return;
        };

        let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
        let mut rx_buf = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut tx_frame = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut seq: u8 = 0;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let mut bitrate_bps = profile.nominal_bitrate_bps();
        let mut duty_cycle = profile.region.duty_cycle();
        let mut gate: DutyGate<FixedDutyQueue<DUTY_QUEUE_FRAMES>> = DutyGate::new();
        let started = Instant::now();
        status.set_connection(ConnectionState::Connected);

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                while !status.is_enabled() {
                    Timer::after(ENABLED_POLL).await;
                }
                status.set_connection(ConnectionState::Connected);
            }

            if let Err(e) = radio
                .prepare_for_rx(RxMode::Continuous, &params.modulation, &params.rx_pkt)
                .await
            {
                log::warn!("RNS_LORA prepare_for_rx failed: {e:?}");
                Timer::after(RX_RETRY_WAIT).await;
                continue;
            }

            match select4(
                radio.rx(&params.rx_pkt, &mut rx_buf),
                seam.next_outbound(),
                control.wait(),
                Timer::after(ENABLED_POLL),
            )
            .await
            {
                Either4::First(Ok((len, _rx_status))) => {
                    let len = len as usize;
                    let now = InstantMillis(started.elapsed().as_millis());
                    status.add_rx(len as u64);
                    throughput.record_rx(now, len as u64);
                    status.set_transfer_rates(throughput.rates());
                    if let Some(packet) = reassembler.feed(&rx_buf[..len]) {
                        if !packet.is_empty() && packet.len() <= LORA_MAX_PAYLOAD {
                            seam.next_inbound(packet).await;
                        }
                    }
                }
                Either4::First(Err(e)) => {
                    log::warn!("RNS_LORA rx error: {e:?}");
                }
                Either4::Second(outbound) => {
                    let now = InstantMillis(started.elapsed().as_millis());
                    let power_dbm = profile.tx_power.dbm() as i32;
                    let transmit = match duty_cycle {
                        None => true,
                        Some(duty) => {
                            let air = packet_airtime(outbound, bitrate_bps);
                            let util = airtime.utilization(now);
                            matches!(gate.offer(outbound, air, util, &duty), DutyVerdict::Transmit)
                        }
                    };
                    if transmit {
                        transmit_packet(
                            &mut radio,
                            &mut params,
                            power_dbm,
                            outbound,
                            &mut seq,
                            &mut airtime,
                            &mut throughput,
                            status,
                            bitrate_bps,
                            now,
                            &mut tx_frame,
                        )
                        .await;
                    }
                }
                Either4::Third(new_profile) => match build_params(&mut radio, &new_profile) {
                    Some(rebuilt) => {
                        params = rebuilt;
                        profile = new_profile;
                        bitrate_bps = profile.nominal_bitrate_bps();
                        duty_cycle = profile.region.duty_cycle();
                        if let Some(message) = retag_message(current_id, &profile) {
                            if let InterfaceLifecycle::Retag { new_id, .. } = &message {
                                current_id = *new_id;
                            }
                            retag.send(message).await;
                        }
                    }
                    None => {
                        log::warn!("RNS_LORA reconfigure to an unsupported modulation ignored");
                    }
                },
                Either4::Fourth(()) => {
                    if let Some(duty) = duty_cycle {
                        let now = InstantMillis(started.elapsed().as_millis());
                        let power_dbm = profile.tx_power.dbm() as i32;
                        let mut released_packet = [0u8; LORA_MAX_PAYLOAD];
                        loop {
                            let util = airtime.utilization(now);
                            let mut released_len = None;
                            let released = gate.release_ready(util, &duty, |bytes| {
                                let len = bytes.len().min(released_packet.len());
                                released_packet[..len].copy_from_slice(&bytes[..len]);
                                released_len = Some(len);
                            });
                            if !released {
                                break;
                            }
                            if let Some(len) = released_len {
                                transmit_packet(
                                    &mut radio,
                                    &mut params,
                                    power_dbm,
                                    &released_packet[..len],
                                    &mut seq,
                                    &mut airtime,
                                    &mut throughput,
                                    status,
                                    bitrate_bps,
                                    now,
                                    &mut tx_frame,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::core::{DEFAULT_915_PROFILE, PreambleSymbols, TxPower};
    use super::*;

    fn id_of(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(profile))
    }

    #[test]
    fn a_channel_change_re_keys_to_the_new_id() {
        let current = id_of(&DEFAULT_915_PROFILE);
        let mut next = DEFAULT_915_PROFILE;
        next.modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf10,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        let message = retag_message(current, &next).expect("a channel change re-keys");
        let InterfaceLifecycle::Retag {
            old_id, new_id, ..
        } = message
        else {
            panic!("expected a Retag");
        };
        assert_eq!(old_id, current);
        assert_eq!(new_id, id_of(&next));
        assert_ne!(new_id, current);
    }

    #[test]
    fn a_local_only_change_does_not_re_key() {
        let current = id_of(&DEFAULT_915_PROFILE);
        let mut next = DEFAULT_915_PROFILE;
        next.tx_power = TxPower::new(2);
        next.preamble = PreambleSymbols::new(24);
        assert!(
            retag_message(current, &next).is_none(),
            "transmit power and preamble are local knobs, not channel identity"
        );
    }

    #[test]
    fn packet_airtime_sums_both_frames_of_a_split() {
        let bitrate = 5_000;
        let one_frame = packet_airtime(&[0u8; 100], bitrate);
        let two_frames = packet_airtime(&[0u8; 400], bitrate);
        assert_eq!(
            one_frame,
            frame_airtime_us(101, bitrate),
            "one frame: the header plus 100 payload bytes on air"
        );
        assert_eq!(
            two_frames,
            frame_airtime_us(255, bitrate) + frame_airtime_us(147, bitrate),
            "two frames: a full 255-byte frame plus the 147-byte remainder"
        );
        assert!(two_frames > one_frame);
    }
}
