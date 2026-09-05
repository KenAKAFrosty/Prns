pub const TURBO_AIR_FRAME_MAX: usize = 255;
pub const TURBO_DATA_HEADER_BYTES: usize = 5;
pub const TURBO_FRAME_DATA_MAX: usize = TURBO_AIR_FRAME_MAX - TURBO_DATA_HEADER_BYTES;
pub const TURBO_LOGICAL_PACKET_MAX: usize = TURBO_FRAME_DATA_MAX * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitRate(u32);

impl BitRate {
    pub const fn from_bps(bps: u32) -> Result<Self, TurboProfileError> {
        if bps == 0 {
            return Err(TurboProfileError::ZeroBitRate);
        }
        Ok(Self(bps))
    }

    pub const fn bps(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrequencyDeviation(u32);

impl FrequencyDeviation {
    pub const fn from_hz(hz: u32) -> Result<Self, TurboProfileError> {
        if hz == 0 {
            return Err(TurboProfileError::ZeroFrequencyDeviation);
        }
        Ok(Self(hz))
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverBandwidth(u32);

impl ReceiverBandwidth {
    pub const fn from_hz(hz: u32) -> Result<Self, TurboProfileError> {
        if hz == 0 {
            return Err(TurboProfileError::ZeroReceiverBandwidth);
        }
        Ok(Self(hz))
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaussianFilter {
    Bt05,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulationIndex {
    Half,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketMode {
    VariableLength,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataWhitening {
    Pn9 { polynomial: u16, seed: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketCrc {
    CcittFalse {
        polynomial: u16,
        initial: u16,
        xor_out: u16,
    },
}

impl PacketCrc {
    const fn bits(self) -> u8 {
        16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboPhyProfile {
    bit_rate: BitRate,
    frequency_deviation: FrequencyDeviation,
    receiver_bandwidth: ReceiverBandwidth,
    gaussian_filter: GaussianFilter,
    modulation_index: ModulationIndex,
    data_whitening: DataWhitening,
    packet_mode: PacketMode,
    preamble_bits: u16,
    sync_word: [u8; 4],
    packet_crc: PacketCrc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboProfileError {
    ZeroBitRate,
    ZeroFrequencyDeviation,
    ZeroReceiverBandwidth,
    ReceiverBandwidthTooNarrow { required_hz: u32, actual_hz: u32 },
    UnsupportedHardwareCapability { capability: TurboPhyCapability },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboPhyCapability {
    BitRate,
    FrequencyDeviation,
    ReceiverBandwidth,
    GaussianFilter,
    ModulationIndex,
    Whitening,
    VariableLengthPackets,
    Preamble,
    SyncWord,
    Crc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboHardwareSupport {
    pub bit_rate: CapabilitySupport,
    pub frequency_deviation: CapabilitySupport,
    pub receiver_bandwidth: CapabilitySupport,
    pub gaussian_filter: CapabilitySupport,
    pub modulation_index: CapabilitySupport,
    pub whitening: CapabilitySupport,
    pub variable_length_packets: CapabilitySupport,
    pub preamble: CapabilitySupport,
    pub sync_word: CapabilitySupport,
    pub crc: CapabilitySupport,
}

impl TurboPhyProfile {
    pub const fn validate(self) -> Result<(), TurboProfileError> {
        let required_hz = self
            .bit_rate
            .bps()
            .saturating_add(self.frequency_deviation.hz().saturating_mul(2));
        if self.receiver_bandwidth.hz() < required_hz {
            return Err(TurboProfileError::ReceiverBandwidthTooNarrow {
                required_hz,
                actual_hz: self.receiver_bandwidth.hz(),
            });
        }
        Ok(())
    }

    pub const fn verify_hardware(
        self,
        support: TurboHardwareSupport,
    ) -> Result<(), TurboProfileError> {
        let capabilities = [
            (support.bit_rate, TurboPhyCapability::BitRate),
            (
                support.frequency_deviation,
                TurboPhyCapability::FrequencyDeviation,
            ),
            (
                support.receiver_bandwidth,
                TurboPhyCapability::ReceiverBandwidth,
            ),
            (support.gaussian_filter, TurboPhyCapability::GaussianFilter),
            (
                support.modulation_index,
                TurboPhyCapability::ModulationIndex,
            ),
            (support.whitening, TurboPhyCapability::Whitening),
            (
                support.variable_length_packets,
                TurboPhyCapability::VariableLengthPackets,
            ),
            (support.preamble, TurboPhyCapability::Preamble),
            (support.sync_word, TurboPhyCapability::SyncWord),
            (support.crc, TurboPhyCapability::Crc),
        ];
        let mut index = 0;
        while index < capabilities.len() {
            if matches!(capabilities[index].0, CapabilitySupport::Unsupported) {
                return Err(TurboProfileError::UnsupportedHardwareCapability {
                    capability: capabilities[index].1,
                });
            }
            index += 1;
        }
        Ok(())
    }

    pub const fn bit_rate(self) -> BitRate {
        self.bit_rate
    }

    pub const fn frequency_deviation(self) -> FrequencyDeviation {
        self.frequency_deviation
    }

    pub const fn receiver_bandwidth(self) -> ReceiverBandwidth {
        self.receiver_bandwidth
    }

    pub const fn gaussian_filter(self) -> GaussianFilter {
        self.gaussian_filter
    }

    pub const fn modulation_index(self) -> ModulationIndex {
        self.modulation_index
    }

    pub const fn data_whitening(self) -> DataWhitening {
        self.data_whitening
    }

    pub const fn packet_mode(self) -> PacketMode {
        self.packet_mode
    }

    pub const fn preamble_bits(self) -> u16 {
        self.preamble_bits
    }

    pub const fn sync_word(self) -> [u8; 4] {
        self.sync_word
    }

    pub const fn packet_crc(self) -> PacketCrc {
        self.packet_crc
    }

    pub const fn time_on_air_us(self, frame_bytes: usize) -> u64 {
        let framed_bits = (self.preamble_bits as u64)
            .saturating_add((self.sync_word.len() as u64).saturating_mul(8))
            .saturating_add(8)
            .saturating_add((frame_bytes as u64).saturating_mul(8))
            .saturating_add(self.packet_crc.bits() as u64);
        framed_bits
            .saturating_mul(1_000_000)
            .div_ceil(self.bit_rate.bps() as u64)
    }

    pub const fn logical_packet_airtime_us(self, packet_bytes: usize) -> u64 {
        if packet_bytes == 0 || packet_bytes > TURBO_LOGICAL_PACKET_MAX {
            return 0;
        }
        if packet_bytes <= TURBO_FRAME_DATA_MAX {
            return self.time_on_air_us(TURBO_DATA_HEADER_BYTES + packet_bytes);
        }
        self.time_on_air_us(TURBO_AIR_FRAME_MAX).saturating_add(
            self.time_on_air_us(TURBO_DATA_HEADER_BYTES + packet_bytes - TURBO_FRAME_DATA_MAX),
        )
    }
}

pub const US915_TURBO_PHY: TurboPhyProfile = TurboPhyProfile {
    bit_rate: BitRate(250_000),
    frequency_deviation: FrequencyDeviation(62_500),
    receiver_bandwidth: ReceiverBandwidth(467_000),
    gaussian_filter: GaussianFilter::Bt05,
    modulation_index: ModulationIndex::Half,
    data_whitening: DataWhitening::Pn9 {
        polynomial: 0x021,
        seed: 0x1ff,
    },
    packet_mode: PacketMode::VariableLength,
    preamble_bits: 32,
    sync_word: *b"PRNS",
    packet_crc: PacketCrc::CcittFalse {
        polynomial: 0x1021,
        initial: 0xffff,
        xor_out: 0,
    },
};
