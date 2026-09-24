use core::fmt::Debug;

use prns_core::interfaces::lora::{LoRaProfile, RadioProfile, RadioProfileCompatibilityError};
use prns_core::interfaces::PacketPhyStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedAirFrame {
    pub len: usize,
    pub phy: PacketPhyStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioEvent {
    PreambleDetected,
    HeaderValid,
    Frame(ReceivedAirFrame),
    HeaderError,
    CrcError,
    Timeout,
    SpuriousInterrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioRecovery {
    Continue,
    Reinitialize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BandRadioError<E> {
    UnsupportedBand,
    Radio(E),
}

#[allow(async_fn_in_trait)]
pub trait LoRaRadio {
    type Error: Debug;

    fn validate_band_profile(
        &self,
        profile: LoRaProfile,
    ) -> Result<(), RadioProfileCompatibilityError> {
        match profile {
            LoRaProfile::SubG(profile) => self.validate_profile(profile),
            #[cfg(feature = "lora-2g4")]
            LoRaProfile::Ghz24(_) => Err(RadioProfileCompatibilityError::UnsupportedBand),
        }
    }

    async fn initialize_band(
        &mut self,
        profile: LoRaProfile,
    ) -> Result<(), BandRadioError<Self::Error>> {
        match profile {
            LoRaProfile::SubG(profile) => self
                .initialize(profile)
                .await
                .map_err(BandRadioError::Radio),
            #[cfg(feature = "lora-2g4")]
            LoRaProfile::Ghz24(_) => Err(BandRadioError::UnsupportedBand),
        }
    }

    fn validate_profile(&self, profile: RadioProfile)
        -> Result<(), RadioProfileCompatibilityError>;

    fn recovery(error: &Self::Error) -> RadioRecovery;

    async fn initialize(&mut self, profile: RadioProfile) -> Result<(), Self::Error>;

    async fn idle(&mut self) -> Result<(), Self::Error>;

    async fn arm_rx(&mut self) -> Result<(), Self::Error>;

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), Self::Error>;

    async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error>;

    async fn read_event(&mut self, buffer: &mut [u8]) -> Result<RadioEvent, Self::Error>;

    async fn poll_event(&mut self, buffer: &mut [u8]) -> Result<Option<RadioEvent>, Self::Error>;
}
