//! The embassy SX1262 worker: owns the radio over our own `subghz_rf` driver, bridges its RX/TX to
//! the reactor seam, and reconfigures the radio in place when the app signals a new profile. Generic
//! over the driver's `embedded-hal-async` SPI + GPIO bounds, so the same body drives a Heltec V4
//! (esp-hal) or an nRF SX1262 board and compile-checks on the host. A reconfigure retunes the
//! silicon and, when it changes the channel identity, emits a `Retag` so the reactor re-keys the
//! interface in place.

use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::DynamicSender;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiDevice;
use heapless::Vec as HeaplessVec;

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::rnode_lora::core::{
    self, air_frame_count, encode_air_frame_part, CodingRate, LoRaReassembler, LoraBandwidth,
    Modulation, RadioProfile, SpreadingFactor, CHANNEL_TAG_CAP, LORA_MAX_PAYLOAD,
    LORA_SINGLE_FRAME_MAX,
};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::duty_gate::{DutyGate, DutyVerdict, FixedDutyQueue};
use crate::reactor::impls::embassy_reactor::{EmbassyInterfaceStatus, InterfaceLifecycle};
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;
use crate::subghz_rf::{self, Sx126x};

/// How often a serving radio re-checks its enabled gate, so a "Power" toggle from the UI takes
/// effect within a beat rather than waiting on traffic.
pub const ENABLED_POLL: Duration = Duration::from_millis(250);
/// How many held packets the duty gate buffers before dropping the oldest. The region's airtime
/// budget caps the queue sooner on a slow link, so this only bounds the buffer's memory.
const DUTY_QUEUE_FRAMES: usize = 4;

/// The signal the app holds to reconfigure a running radio: it sends a whole new [`RadioProfile`]
/// and the worker rebuilds the silicon's params from it. The granular, menu-shaped control surface
/// (set-frequency, set-modulation, …) lands with the reconfigure arc.
pub type LoRaControl = Signal<CriticalSectionRawMutex, RadioProfile>;

/// Map a profile's LoRa channel onto our `subghz_rf` driver's `init` arguments —
/// `(frequency_hz, modulation, packet shape, tx power dBm)`. `None` when the profile is GFSK: the
/// driver's LoRa arm is all that's wired today, so the speed mode waits on its own path.
fn subghz_params(
    profile: &RadioProfile,
) -> Option<(u32, subghz_rf::Modulation, subghz_rf::LoraPacket, i8)> {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation
    else {
        return None;
    };
    let spreading_factor = match spreading_factor {
        SpreadingFactor::Sf5 => subghz_rf::SpreadingFactor::Sf5,
        SpreadingFactor::Sf6 => subghz_rf::SpreadingFactor::Sf6,
        SpreadingFactor::Sf7 => subghz_rf::SpreadingFactor::Sf7,
        SpreadingFactor::Sf8 => subghz_rf::SpreadingFactor::Sf8,
        SpreadingFactor::Sf9 => subghz_rf::SpreadingFactor::Sf9,
        SpreadingFactor::Sf10 => subghz_rf::SpreadingFactor::Sf10,
        SpreadingFactor::Sf11 => subghz_rf::SpreadingFactor::Sf11,
        SpreadingFactor::Sf12 => subghz_rf::SpreadingFactor::Sf12,
    };
    let bandwidth = match bandwidth {
        LoraBandwidth::Bw125kHz => subghz_rf::Bandwidth::Bw125,
        LoraBandwidth::Bw250kHz => subghz_rf::Bandwidth::Bw250,
        LoraBandwidth::Bw500kHz => subghz_rf::Bandwidth::Bw500,
    };
    let coding_rate = match coding_rate {
        CodingRate::Cr45 => subghz_rf::CodingRate::Cr4_5,
        CodingRate::Cr46 => subghz_rf::CodingRate::Cr4_6,
        CodingRate::Cr47 => subghz_rf::CodingRate::Cr4_7,
        CodingRate::Cr48 => subghz_rf::CodingRate::Cr4_8,
    };
    let packet = subghz_rf::LoraPacket {
        preamble_symbols: profile.preamble.count(),
        explicit_header: true,
        crc_on: true,
        invert_iq: false,
    };
    Some((
        profile.frequency.hz(),
        subghz_rf::Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        packet,
        profile.tx_power.dbm(),
    ))
}

/// The [`Retag`](InterfaceLifecycle::Retag) a reconfigure to `new_profile` warrants, or `None` when
/// the change leaves the channel identity untouched — a local knob like transmit power or preamble.
/// The channel_tag (frequency + modulation) is what mints the id, so only a change to it re-keys.
fn retag_message(
    current_id: InterfaceId,
    new_profile: &RadioProfile,
) -> Option<InterfaceLifecycle> {
    let new_id =
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(new_profile));
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
async fn transmit_packet<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    packet: &[u8],
    seq: &mut u8,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    status: &EmbassyInterfaceStatus,
    bitrate_bps: u32,
    now: InstantMillis,
    tx_frame: &mut [u8; LORA_SINGLE_FRAME_MAX],
) where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    for index in 0..air_frame_count(packet.len()) {
        let n = match encode_air_frame_part(packet, *seq, index, tx_frame) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("RNS_LORA frame {index} encode failed: {e:?}");
                break;
            }
        };
        // TX power, modulation, and packet shape are held in the radio from `init`; only the
        // framed payload changes per frame.
        if let Err(e) = radio.transmit(&tx_frame[..n]).await {
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
pub struct LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    id: InterfaceId,
    radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    profile: RadioProfile,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    control: &'a LoRaControl,
    status: &'a EmbassyInterfaceStatus,
    retag: DynamicSender<'a, InterfaceLifecycle>,
}

impl<'a, SPI, BUSY, DIO1, RST, DLY> LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    /// The id a radio on `profile` will carry — for the caller that stands its
    /// [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(profile))
    }

    #[must_use]
    pub fn new(
        radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
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

impl<SPI, BUSY, DIO1, RST, DLY> Interface for LoRaInterface<'_, SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
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

        let Some((frequency_hz, modulation, packet, power_dbm)) = subghz_params(&profile) else {
            log::error!("RNS_LORA unsupported modulation profile; interface offline");
            status.set_connection(ConnectionState::Disconnected);
            return;
        };
        if let Err(e) = radio
            .init(frequency_hz, modulation, packet, power_dbm)
            .await
        {
            log::error!("RNS_LORA radio init failed: {e:?}; interface offline");
            status.set_connection(ConnectionState::Disconnected);
            return;
        }

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

            match select4(
                radio.receive(&mut rx_buf),
                seam.next_outbound(),
                control.wait(),
                Timer::after(ENABLED_POLL),
            )
            .await
            {
                Either4::First(Ok(len)) => {
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
                    let transmit = match duty_cycle {
                        None => true,
                        Some(duty) => {
                            let air = packet_airtime(outbound, bitrate_bps);
                            let util = airtime.utilization(now);
                            matches!(
                                gate.offer(outbound, air, util, &duty),
                                DutyVerdict::Transmit
                            )
                        }
                    };
                    if transmit {
                        transmit_packet(
                            &mut radio,
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
                Either4::Third(new_profile) => match subghz_params(&new_profile) {
                    Some((frequency_hz, modulation, packet, power_dbm)) => {
                        if let Err(e) = radio
                            .init(frequency_hz, modulation, packet, power_dbm)
                            .await
                        {
                            log::warn!("RNS_LORA reconfigure init failed: {e:?}");
                        } else {
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
                    }
                    None => {
                        log::warn!("RNS_LORA reconfigure to an unsupported modulation ignored");
                    }
                },
                Either4::Fourth(()) => {
                    if let Some(duty) = duty_cycle {
                        let now = InstantMillis(started.elapsed().as_millis());
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
    use super::core::{PreambleSymbols, TxPower, DEFAULT_915_PROFILE};
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
        let InterfaceLifecycle::Retag { old_id, new_id, .. } = message else {
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
