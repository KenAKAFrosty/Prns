use super::{RemoteControlLoRaProfile, RemoteControlMessageWriteError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRadioConfiguration {
    Unconfigured,
    Profile(RemoteControlLoRaProfile),
}

impl RemoteControlRadioConfiguration {
    pub const MAX_ENCODED_LEN: usize = 1 + super::REMOTE_CONTROL_INTERFACE_CONFIG_CAP;
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Unconfigured => 1,
            Self::Profile(profile) => profile.encoded_body_len(),
        }
    }
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let (&length, text) = bytes.split_first()?;
        if usize::from(length) != text.len() {
            return None;
        }
        if length == 0 {
            return Some(Self::Unconfigured);
        }
        Some(Self::Profile(RemoteControlLoRaProfile::parse(
            core::str::from_utf8(text).ok()?,
        )?))
    }
    pub(crate) fn write(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        let out = out
            .get_mut(..self.encoded_len())
            .ok_or(RemoteControlMessageWriteError::BufferTooShort)?;
        let (length, text) = out
            .split_first_mut()
            .ok_or(RemoteControlMessageWriteError::BufferTooShort)?;
        match self {
            Self::Unconfigured => *length = 0,
            Self::Profile(profile) => {
                *length = u8::try_from(profile.as_bytes().len())
                    .map_err(|_| RemoteControlMessageWriteError::BufferTooShort)?;
                text.copy_from_slice(profile.as_bytes());
            }
        }
        Ok(())
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlRadioOutcome {
        Saved = 1,
        UnknownInterface = 2,
        HardwareFailed = 3,
        PersistenceFailed = 4,
        RecoveryRequired = 5,
        Busy = 6,
        IdentityExhausted = 7,
        PublicationFailed = 8,
        InvalidConfiguration = 9,
    }
}
impl RemoteControlRadioOutcome {
    pub const ENCODED_LEN: usize = 1;
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let [value] = bytes else {
            return None;
        };
        Self::ALL
            .iter()
            .copied()
            .find(|outcome| *outcome as u8 == *value)
    }
    pub(crate) fn write(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        *out.first_mut()
            .ok_or(RemoteControlMessageWriteError::BufferTooShort)? = self as u8;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RemoteControlRadioBands {
    SubG = 1,
    SubGAndGhz24 = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RemoteControlRadioOperatingState {
    Unconfigured = 0,
    Disabled = 1,
    Operating = 2,
    Failed = 3,
    Changing = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRadioSaved {
    Unknown,
    Confirmed(RemoteControlRadioConfiguration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRadioStatus {
    UnknownInterface,
    Status {
        bands: RemoteControlRadioBands,
        operating: RemoteControlRadioOperatingState,
        saved: RemoteControlRadioSaved,
    },
}
impl RemoteControlRadioStatus {
    pub const MAX_ENCODED_LEN: usize = 4 + RemoteControlRadioConfiguration::MAX_ENCODED_LEN;
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::UnknownInterface => 1,
            Self::Status {
                saved: RemoteControlRadioSaved::Unknown,
                ..
            } => 4,
            Self::Status {
                saved: RemoteControlRadioSaved::Confirmed(configuration),
                ..
            } => 4usize.saturating_add(configuration.encoded_len()),
        }
    }
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes == [0] {
            return Some(Self::UnknownInterface);
        }
        let [1, bands, operating, saved, rest @ ..] = bytes else {
            return None;
        };
        let bands = match bands {
            1 => RemoteControlRadioBands::SubG,
            3 => RemoteControlRadioBands::SubGAndGhz24,
            _ => return None,
        };
        let operating = match operating {
            0 => RemoteControlRadioOperatingState::Unconfigured,
            1 => RemoteControlRadioOperatingState::Disabled,
            2 => RemoteControlRadioOperatingState::Operating,
            3 => RemoteControlRadioOperatingState::Failed,
            4 => RemoteControlRadioOperatingState::Changing,
            _ => return None,
        };
        let saved = match saved {
            0 if rest.is_empty() => RemoteControlRadioSaved::Unknown,
            1 => RemoteControlRadioSaved::Confirmed(RemoteControlRadioConfiguration::parse(rest)?),
            _ => return None,
        };
        Some(Self::Status {
            bands,
            operating,
            saved,
        })
    }
    pub(crate) fn write(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        let (bands, operating, saved) = match self {
            Self::UnknownInterface => {
                *out.first_mut()
                    .ok_or(RemoteControlMessageWriteError::BufferTooShort)? = 0;
                return Ok(());
            }
            Self::Status {
                bands,
                operating,
                saved,
            } => (bands, operating, saved),
        };
        let [tag, bands_out, operating_out, saved_out, rest @ ..] = out else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *tag = 1;
        *bands_out = bands as u8;
        *operating_out = operating as u8;
        match saved {
            RemoteControlRadioSaved::Unknown => *saved_out = 0,
            RemoteControlRadioSaved::Confirmed(configuration) => {
                *saved_out = 1;
                configuration.write(rest)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{subghz::regions::us915::US915_AUTO_LORA_PROFILE, InterfaceId};
    use crate::remote_control::{RemoteControlRequest, RemoteControlResponse};

    #[test]
    fn radio_message_capacities_preserve_the_embedded_wire_buffer_budget() {
        // One counted 48-byte profile; status adds presence, band, operating and saved tags.
        assert_eq!(RemoteControlRadioConfiguration::MAX_ENCODED_LEN, 49);
        assert_eq!(RemoteControlRadioStatus::MAX_ENCODED_LEN, 53);
    }

    #[test]
    fn radio_commands_round_trip_with_exact_bounds() {
        let id = InterfaceId::new(*b"testlora");
        let profile = RemoteControlLoRaProfile::from_profile(US915_AUTO_LORA_PROFILE).unwrap();
        let configurations = [
            RemoteControlRadioConfiguration::Unconfigured,
            RemoteControlRadioConfiguration::Profile(profile),
        ];
        for configuration in configurations {
            for request in [
                RemoteControlRequest::InspectRadio { id },
                RemoteControlRequest::ConfigureRadio { id, configuration },
            ] {
                let mut bytes = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
                let len = request.write_into(&mut bytes).unwrap();
                assert_eq!(
                    RemoteControlRequest::parse(bytes.get(..len).unwrap()).as_ref(),
                    Ok(&request)
                );
                assert!(request
                    .write_into(bytes.get_mut(..len - 1).unwrap())
                    .is_err());
                for prefix in 0..len {
                    assert!(RemoteControlRequest::parse(bytes.get(..prefix).unwrap()).is_err());
                }
            }
        }
        assert_eq!(RemoteControlRadioConfiguration::parse(&[0, 0]), None);
        assert_eq!(RemoteControlRadioConfiguration::parse(&[1, 255]), None);
        assert_eq!(RemoteControlRadioConfiguration::parse(&[1, b'X']), None);
    }

    #[test]
    fn radio_status_and_outcomes_round_trip_without_collapsing_unknown_state() {
        let profile = RemoteControlLoRaProfile::from_profile(US915_AUTO_LORA_PROFILE).unwrap();
        for bands in [
            RemoteControlRadioBands::SubG,
            RemoteControlRadioBands::SubGAndGhz24,
        ] {
            for operating in [
                RemoteControlRadioOperatingState::Unconfigured,
                RemoteControlRadioOperatingState::Disabled,
                RemoteControlRadioOperatingState::Operating,
                RemoteControlRadioOperatingState::Failed,
                RemoteControlRadioOperatingState::Changing,
            ] {
                for saved in [
                    RemoteControlRadioSaved::Unknown,
                    RemoteControlRadioSaved::Confirmed(
                        RemoteControlRadioConfiguration::Unconfigured,
                    ),
                    RemoteControlRadioSaved::Confirmed(RemoteControlRadioConfiguration::Profile(
                        profile,
                    )),
                ] {
                    let status = RemoteControlRadioStatus::Status {
                        bands,
                        operating,
                        saved,
                    };
                    let response = RemoteControlResponse::InspectRadio(status);
                    let mut bytes = [0; 128];
                    let len = response.write_into(&mut bytes).unwrap();
                    assert_eq!(
                        RemoteControlResponse::parse(bytes.get(..len).unwrap()),
                        Ok(response)
                    );
                    for prefix in 0..status.encoded_len() {
                        assert!(status.write(bytes.get_mut(..prefix).unwrap()).is_err());
                    }
                }
            }
        }
        for outcome in RemoteControlRadioOutcome::ALL {
            let response = RemoteControlResponse::ConfigureRadio(outcome);
            let mut bytes = [0; 3];
            assert_eq!(response.write_into(&mut bytes), Ok(3));
            assert_eq!(RemoteControlResponse::parse(&bytes), Ok(response));
            assert!(outcome.write(&mut []).is_err());
        }
        assert_eq!(RemoteControlRadioOutcome::parse(&[]), None);
        assert_eq!(RemoteControlRadioOutcome::parse(&[0]), None);
        assert_eq!(RemoteControlRadioOutcome::parse(&[1, 1]), None);
        let unknown = RemoteControlRadioStatus::UnknownInterface;
        assert_eq!(unknown.encoded_len(), 1);
        assert!(unknown.write(&mut []).is_err());
        let mut bytes = [255];
        assert_eq!(unknown.write(&mut bytes), Ok(()));
        assert_eq!(RemoteControlRadioStatus::parse(&bytes), Some(unknown));
        for bytes in [
            &[][..],
            &[0, 0],
            &[1, 0, 0, 0],
            &[1, 1, 5, 0],
            &[1, 1, 0, 2],
            &[1, 1, 0, 0, 0],
            &[1, 1, 0, 1],
        ] {
            assert_eq!(RemoteControlRadioStatus::parse(bytes), None);
        }
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(12)]
    fn radio_outcome_codec_accepts_exactly_its_documented_tags() {
        let bytes: [u8; 2] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let parsed = RemoteControlRadioOutcome::parse(&bytes[..len]);
        assert_eq!(parsed.is_some(), len == 1 && (1..=9).contains(&bytes[0]));
        if let Some(outcome) = parsed {
            let mut encoded = [0];
            assert!(outcome.write(&mut encoded).is_ok());
            assert_eq!(encoded[0], bytes[0]);
        }
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn radio_status_prefix_preserves_capability_and_operating_state_tags() {
        let bands: u8 = kani::any();
        let operating: u8 = kani::any();
        let bytes = [1, bands, operating, 0];
        let parsed = RemoteControlRadioStatus::parse(&bytes);
        assert_eq!(parsed.is_some(), matches!(bands, 1 | 3) && operating <= 4);
        if let Some(status) = parsed {
            let mut encoded = [0; 4];
            assert!(status.write(&mut encoded).is_ok());
            assert_eq!(encoded, bytes);
        }
    }
}
