use clap::{Args, ValueEnum};
use personal_hopspot_core::{
    encode_provisioned_auto_announce, encode_provisioned_node_announce_name,
    encode_provisioned_radio_profile, NodeAnnounceName, NodeAnnounceNameError,
    AUTO_ANNOUNCE_EXTENSION_OFFSET, NODE_ANNOUNCE_NAME_EXTENSION_OFFSET, RADIO_PROFILE_RECORD_LEN,
    S3_8_MIB_FLASH_LAYOUT,
};
use personal_rns::interfaces::lora::{
    CodingRate, Frequency, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, Region,
    SpreadingFactor, TxPower, DEFAULT_915_PROFILE,
};

use crate::error::AppError;

const XIAO_ESP32S3_WIO_SX1262: &str = "xiao-esp32s3-wio-sx1262";

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RadioProfileArgs {
    /// Automatically announce the Hopspot node at this interval in minutes.
    #[arg(long = "announce-mm", value_name = "MINUTES", value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) announce_minutes: Option<u32>,
    /// Public display name announced for the built-in NomadNet node.
    #[arg(long = "node-name", value_name = "NAME")]
    pub(crate) node_name: Option<String>,
    /// Regulatory region used to validate frequency, power, and duty-cycle limits.
    #[arg(long, value_enum)]
    region: Option<RegionArg>,

    /// LoRa carrier frequency in hertz.
    #[arg(long, value_name = "HZ")]
    frequency_hz: Option<u32>,

    /// LoRa spreading factor (5 through 12).
    #[arg(long, value_name = "SF", value_parser = clap::value_parser!(u8).range(5..=12))]
    spreading_factor: Option<u8>,

    /// LoRa transmit power in dBm.
    #[arg(long, value_name = "DBM", allow_hyphen_values = true)]
    tx_power_dbm: Option<i8>,

    /// LoRa bandwidth in kHz (125, 250, or 500).
    #[arg(long, value_name = "KHZ", value_parser = ["125", "250", "500"])]
    bandwidth_khz: Option<String>,

    /// LoRa coding rate, expressed as 4/5, 4/6, 4/7, or 4/8.
    #[arg(long, value_name = "RATE", value_parser = ["4/5", "4/6", "4/7", "4/8"])]
    coding_rate: Option<String>,

    /// LoRa preamble length in symbols.
    #[arg(long, value_name = "SYMBOLS", value_parser = clap::value_parser!(u16).range(1..))]
    preamble: Option<u16>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RegionArg {
    Us915,
    Au915,
    Eu433,
    Eu865,
    Eu868,
    Eu869,
    As923,
    In865,
    Cn470,
    Kr920,
    Jp920,
}

impl From<RegionArg> for Region {
    fn from(value: RegionArg) -> Self {
        match value {
            RegionArg::Us915 => Self::Us915,
            RegionArg::Au915 => Self::Au915,
            RegionArg::Eu433 => Self::Eu433,
            RegionArg::Eu865 => Self::Eu865,
            RegionArg::Eu868 => Self::Eu868,
            RegionArg::Eu869 => Self::Eu869,
            RegionArg::As923 => Self::As923,
            RegionArg::In865 => Self::In865,
            RegionArg::Cn470 => Self::Cn470,
            RegionArg::Kr920 => Self::Kr920,
            RegionArg::Jp920 => Self::Jp920,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProvisionedRadioProfile {
    pub(crate) offset: u32,
    pub(crate) bytes: Vec<u8>,
    pub(crate) summary: String,
}

impl RadioProfileArgs {
    pub(crate) fn required_provisioned_for(
        &self,
        board_slug: &str,
    ) -> Result<ProvisionedRadioProfile, AppError> {
        self.provisioned_for(board_slug)?.ok_or_else(|| {
            AppError::arguments("configure requires --region and the desired LoRa profile options")
        })
    }

    pub(crate) fn provisioned_for(
        &self,
        board_slug: &str,
    ) -> Result<Option<ProvisionedRadioProfile>, AppError> {
        if (self.announce_minutes.is_some() || self.node_name.is_some()) && !self.radio_requested()
        {
            return Err(AppError::arguments(
                "--announce-mm and --node-name require a LoRa profile; include --region (and any desired overrides)",
            ));
        }
        if !self.requested() {
            return Ok(None);
        }
        if board_slug != XIAO_ESP32S3_WIO_SX1262 {
            return Err(AppError::arguments(format!(
                "radio profile flags are currently supported only for {XIAO_ESP32S3_WIO_SX1262}"
            )));
        }
        let region = self.region.ok_or_else(|| {
            AppError::arguments("--region is required when provisioning a radio profile")
        })?;
        let region: Region = region.into();
        let Modulation::Lora {
            spreading_factor: default_spreading_factor,
            bandwidth: default_bandwidth,
            coding_rate: default_coding_rate,
        } = DEFAULT_915_PROFILE.modulation;
        let spreading_factor = self
            .spreading_factor
            .map(parse_spreading_factor)
            .transpose()?
            .unwrap_or(default_spreading_factor);
        let bandwidth = self
            .bandwidth_khz
            .as_deref()
            .map(parse_bandwidth)
            .transpose()?
            .unwrap_or(default_bandwidth);
        let coding_rate = self
            .coding_rate
            .as_deref()
            .map(parse_coding_rate)
            .transpose()?
            .unwrap_or(default_coding_rate);
        let profile = RadioProfile {
            frequency: Frequency::new(
                self.frequency_hz
                    .unwrap_or_else(|| region.default_frequency().hz()),
            ),
            modulation: Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate,
            },
            tx_power: TxPower::new(
                self.tx_power_dbm
                    .unwrap_or_else(|| region.max_tx_power().dbm()),
            ),
            preamble: PreambleSymbols::new(
                self.preamble
                    .unwrap_or_else(|| DEFAULT_915_PROFILE.preamble.count()),
            ),
            region,
        };
        let record = encode_provisioned_radio_profile(profile).map_err(|error| {
            AppError::arguments(format!("invalid LoRa radio profile: {error:?}"))
        })?;
        let pages = S3_8_MIB_FLASH_LAYOUT.radio_profile_pages;
        let page_size = usize::try_from(pages[1] - pages[0])
            .map_err(|_| AppError::configuration("invalid radio-profile page layout"))?;
        let mut bytes = vec![0xFF; page_size * 2];
        bytes[..RADIO_PROFILE_RECORD_LEN].copy_from_slice(&record);
        if let Some(minutes) = self.announce_minutes {
            let extension = encode_provisioned_auto_announce(Some(minutes));
            bytes[AUTO_ANNOUNCE_EXTENSION_OFFSET..AUTO_ANNOUNCE_EXTENSION_OFFSET + extension.len()]
                .copy_from_slice(&extension);
        }
        if let Some(name) = &self.node_name {
            let name = NodeAnnounceName::new(name).map_err(|error| match error {
                NodeAnnounceNameError::Empty => {
                    AppError::arguments("--node-name must not be empty")
                }
                NodeAnnounceNameError::TooLong { actual } => AppError::arguments(format!(
                    "--node-name is {actual} UTF-8 bytes; maximum is 64"
                )),
            })?;
            let extension = encode_provisioned_node_announce_name(Some(name));
            bytes[NODE_ANNOUNCE_NAME_EXTENSION_OFFSET
                ..NODE_ANNOUNCE_NAME_EXTENSION_OFFSET + extension.len()]
                .copy_from_slice(&extension);
        }
        Ok(Some(ProvisionedRadioProfile {
            offset: pages[0],
            bytes,
            summary: format!(
                "{}: {} Hz, SF{}, {} kHz, 4/{}, {} dBm, preamble {}",
                region.label(),
                profile.frequency.hz(),
                spreading_factor as u8,
                bandwidth.hz() / 1_000,
                coding_rate.denominator(),
                profile.tx_power.dbm(),
                profile.preamble.count(),
            ),
        }))
    }

    fn requested(&self) -> bool {
        self.radio_requested() || self.node_name.is_some()
    }

    fn radio_requested(&self) -> bool {
        self.region.is_some()
            || self.frequency_hz.is_some()
            || self.spreading_factor.is_some()
            || self.tx_power_dbm.is_some()
            || self.bandwidth_khz.is_some()
            || self.coding_rate.is_some()
            || self.preamble.is_some()
    }
}

fn parse_spreading_factor(value: u8) -> Result<SpreadingFactor, AppError> {
    match value {
        5 => Ok(SpreadingFactor::Sf5),
        6 => Ok(SpreadingFactor::Sf6),
        7 => Ok(SpreadingFactor::Sf7),
        8 => Ok(SpreadingFactor::Sf8),
        9 => Ok(SpreadingFactor::Sf9),
        10 => Ok(SpreadingFactor::Sf10),
        11 => Ok(SpreadingFactor::Sf11),
        12 => Ok(SpreadingFactor::Sf12),
        _ => Err(AppError::arguments(
            "spreading factor must be between 5 and 12",
        )),
    }
}

fn parse_bandwidth(value: &str) -> Result<LoraBandwidth, AppError> {
    match value {
        "125" => Ok(LoraBandwidth::Bw125kHz),
        "250" => Ok(LoraBandwidth::Bw250kHz),
        "500" => Ok(LoraBandwidth::Bw500kHz),
        _ => Err(AppError::arguments(
            "bandwidth must be 125, 250, or 500 kHz",
        )),
    }
}

fn parse_coding_rate(value: &str) -> Result<CodingRate, AppError> {
    match value {
        "4/5" => Ok(CodingRate::Cr45),
        "4/6" => Ok(CodingRate::Cr46),
        "4/7" => Ok(CodingRate::Cr47),
        "4/8" => Ok(CodingRate::Cr48),
        _ => Err(AppError::arguments(
            "coding rate must be 4/5, 4/6, 4/7, or 4/8",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eu869_overrides_encode_for_xiao() {
        let args = RadioProfileArgs {
            announce_minutes: Some(15),
            node_name: None,
            region: Some(RegionArg::Eu869),
            frequency_hz: Some(869_527_000),
            spreading_factor: Some(9),
            tx_power_dbm: Some(14),
            bandwidth_khz: Some("250".to_string()),
            coding_rate: Some("4/5".to_string()),
            preamble: None,
        };
        let provisioned = args
            .provisioned_for(XIAO_ESP32S3_WIO_SX1262)
            .expect("valid profile")
            .expect("requested profile");
        assert_eq!(
            provisioned.offset,
            S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[0]
        );
        assert_eq!(provisioned.bytes.len(), 8_192);
        assert!(
            provisioned.bytes[RADIO_PROFILE_RECORD_LEN..AUTO_ANNOUNCE_EXTENSION_OFFSET]
                .iter()
                .all(|byte| *byte == 0xFF)
        );
        assert!(provisioned.summary.contains("EU869"));
        assert_eq!(
            &provisioned.bytes[AUTO_ANNOUNCE_EXTENSION_OFFSET..AUTO_ANNOUNCE_EXTENSION_OFFSET + 8],
            &[b'H', b'S', b'A', b'N', 15, 0, 0, 0]
        );
    }

    #[test]
    fn rejects_frequency_outside_selected_region() {
        let args = RadioProfileArgs {
            region: Some(RegionArg::Eu868),
            frequency_hz: Some(869_527_000),
            ..RadioProfileArgs::default()
        };
        assert!(args.provisioned_for(XIAO_ESP32S3_WIO_SX1262).is_err());
    }

    #[test]
    fn configure_requires_a_radio_profile() {
        let error = RadioProfileArgs::default()
            .required_provisioned_for(XIAO_ESP32S3_WIO_SX1262)
            .expect_err("empty configure profile must be rejected");

        assert!(error.to_string().contains("configure requires --region"));
    }
}
