use alloc::{vec, vec::Vec};

use thiserror::Error;

use crate::firmware_crc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DfuDeviceType(u16);

impl DfuDeviceType {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DfuDeviceRevision(u16);

impl DfuDeviceRevision {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationVersion {
    NotEnforced,
    Monotonic(u32),
}

impl ApplicationVersion {
    const fn wire_value(self) -> u32 {
        match self {
            Self::NotEnforced => u32::MAX,
            Self::Monotonic(version) => version,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftdeviceFirmwareId(u16);

impl SoftdeviceFirmwareId {
    pub const fn new(value: u16) -> Result<Self, DfuImageError> {
        if value == 0xfffe {
            Err(DfuImageError::WildcardSoftdeviceRequirement)
        } else {
            Ok(Self(value))
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoftdeviceRequirements {
    values: Vec<SoftdeviceFirmwareId>,
}

impl SoftdeviceRequirements {
    pub fn new(
        required: SoftdeviceFirmwareId,
        additional: impl IntoIterator<Item = SoftdeviceFirmwareId>,
    ) -> Result<Self, DfuImageError> {
        let mut values = vec![required];
        values.extend(additional);
        let maximum = usize::from(u16::MAX);
        if values.len() > maximum {
            return Err(DfuImageError::TooManySoftdeviceRequirements {
                actual: values.len(),
                maximum,
            });
        }
        values.sort_by_key(|value| value.0);
        values.dedup();
        Ok(Self { values })
    }

    pub fn as_slice(&self) -> &[SoftdeviceFirmwareId] {
        &self.values
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationInitPacket {
    bytes: Vec<u8>,
    firmware_crc: crate::FirmwareCrc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationInitPacketSpec {
    pub device_type: DfuDeviceType,
    pub device_revision: DfuDeviceRevision,
    pub application_version: ApplicationVersion,
    pub softdevices: SoftdeviceRequirements,
}

impl ApplicationInitPacket {
    pub fn build(firmware: &[u8], spec: &ApplicationInitPacketSpec) -> Result<Self, DfuImageError> {
        sanity_check_firmware_size(firmware)?;

        let mut bytes = Vec::with_capacity(12 + spec.softdevices.values.len() * 2);
        bytes.extend_from_slice(&spec.device_type.0.to_le_bytes());
        bytes.extend_from_slice(&spec.device_revision.0.to_le_bytes());
        bytes.extend_from_slice(&spec.application_version.wire_value().to_le_bytes());
        bytes.extend_from_slice(&(spec.softdevices.values.len() as u16).to_le_bytes());
        for softdevice in &spec.softdevices.values {
            bytes.extend_from_slice(&softdevice.0.to_le_bytes());
        }
        let firmware_crc = firmware_crc(firmware);
        bytes.extend_from_slice(&firmware_crc.get().to_le_bytes());
        Ok(Self {
            bytes,
            firmware_crc,
        })
    }

    pub fn from_artifacts(
        firmware: &[u8],
        init_packet: &[u8],
        spec: &ApplicationInitPacketSpec,
    ) -> Result<Self, DfuImageError> {
        let expected = Self::build(firmware, spec)?;
        if init_packet.len() != expected.bytes.len() {
            return Err(DfuImageError::InitPacketLengthMismatch {
                expected: expected.bytes.len(),
                actual: init_packet.len(),
            });
        }

        let firmware_crc_offset = init_packet.len() - size_of::<u16>();
        for (offset, (actual, expected)) in init_packet[..firmware_crc_offset]
            .iter()
            .zip(&expected.bytes)
            .enumerate()
        {
            if actual != expected {
                return Err(DfuImageError::InitPacketMetadataMismatch {
                    offset,
                    expected: *expected,
                    actual: *actual,
                });
            }
        }

        let init_packet_crc = u16::from_le_bytes([
            init_packet[firmware_crc_offset],
            init_packet[firmware_crc_offset + 1],
        ]);
        if init_packet_crc != expected.firmware_crc.get() {
            return Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc,
                firmware_crc: expected.firmware_crc.get(),
            });
        }

        Ok(Self {
            bytes: init_packet.to_vec(),
            firmware_crc: expected.firmware_crc,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfuImage<'a> {
    firmware: FirmwareBytes<'a>,
    init_packet: ApplicationInitPacket,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FirmwareBytes<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl FirmwareBytes<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

impl<'a> DfuImage<'a> {
    pub fn new(
        firmware: &'a [u8],
        init_packet: ApplicationInitPacket,
    ) -> Result<Self, DfuImageError> {
        sanity_check_firmware_size(firmware)?;
        let actual_firmware_crc = firmware_crc(firmware);
        if init_packet.firmware_crc != actual_firmware_crc {
            return Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: init_packet.firmware_crc.get(),
                firmware_crc: actual_firmware_crc.get(),
            });
        }
        Ok(Self {
            firmware: FirmwareBytes::Borrowed(firmware),
            init_packet,
        })
    }

    pub fn from_artifacts(
        firmware: &'a [u8],
        init_packet: &[u8],
        spec: &ApplicationInitPacketSpec,
    ) -> Result<Self, DfuImageError> {
        let init_packet = ApplicationInitPacket::from_artifacts(firmware, init_packet, spec)?;
        Self::new(firmware, init_packet)
    }

    pub fn firmware(&self) -> &[u8] {
        self.firmware.as_slice()
    }

    pub fn init_packet(&self) -> &ApplicationInitPacket {
        &self.init_packet
    }
}

impl DfuImage<'static> {
    pub fn new_owned(
        firmware: Vec<u8>,
        init_packet: ApplicationInitPacket,
    ) -> Result<Self, DfuImageError> {
        sanity_check_firmware_size(&firmware)?;
        let actual_firmware_crc = firmware_crc(&firmware);
        if init_packet.firmware_crc != actual_firmware_crc {
            return Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: init_packet.firmware_crc.get(),
                firmware_crc: actual_firmware_crc.get(),
            });
        }
        Ok(Self {
            firmware: FirmwareBytes::Owned(firmware),
            init_packet,
        })
    }

    pub fn from_owned_artifacts(
        firmware: Vec<u8>,
        init_packet: Vec<u8>,
        spec: &ApplicationInitPacketSpec,
    ) -> Result<Self, DfuImageError> {
        let init_packet = ApplicationInitPacket::from_artifacts(&firmware, &init_packet, spec)?;
        Self::new_owned(firmware, init_packet)
    }
}

fn sanity_check_firmware_size(firmware: &[u8]) -> Result<(), DfuImageError> {
    if firmware.is_empty() {
        return Err(DfuImageError::EmptyFirmware);
    }
    if firmware.len() > u32::MAX as usize {
        return Err(DfuImageError::FirmwareTooLarge {
            actual: firmware.len(),
            maximum: u32::MAX as usize,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DfuImageError {
    #[error("DFU firmware image is empty")]
    EmptyFirmware,
    #[error("DFU firmware image is {actual} bytes; the maximum is {maximum}")]
    FirmwareTooLarge { actual: usize, maximum: usize },
    #[error("DFU image declares {actual} SoftDevice requirements; the maximum is {maximum}")]
    TooManySoftdeviceRequirements { actual: usize, maximum: usize },
    #[error("DFU SoftDevice compatibility must name an exact FWID, not the 0xfffe wildcard")]
    WildcardSoftdeviceRequirement,
    #[error("DFU init packet is {actual} bytes; expected exactly {expected}")]
    InitPacketLengthMismatch { expected: usize, actual: usize },
    #[error(
        "DFU init packet metadata differs at byte {offset}: expected 0x{expected:02x}, found 0x{actual:02x}"
    )]
    InitPacketMetadataMismatch {
        offset: usize,
        expected: u8,
        actual: u8,
    },
    #[error(
        "DFU init packet firmware CRC 0x{init_packet_crc:04x} does not match image CRC 0x{firmware_crc:04x}"
    )]
    InitPacketFirmwareMismatch {
        init_packet_crc: u16,
        firmware_crc: u16,
    },
}

#[cfg(test)]
mod tests {
    use std::vec;

    use super::{
        ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuDeviceRevision,
        DfuDeviceType, DfuImage, DfuImageError, SoftdeviceFirmwareId, SoftdeviceRequirements,
    };

    fn t1000e_spec() -> Result<ApplicationInitPacketSpec, DfuImageError> {
        let fwid = SoftdeviceFirmwareId::new(0x0123)?;
        Ok(ApplicationInitPacketSpec {
            device_type: DfuDeviceType::new(0x0052),
            device_revision: DfuDeviceRevision::new(52840),
            application_version: ApplicationVersion::NotEnforced,
            softdevices: SoftdeviceRequirements::new(fwid, std::iter::empty())?,
        })
    }

    #[test]
    fn init_packet_matches_adafruit_nrfutil_reference() -> Result<(), DfuImageError> {
        let packet = ApplicationInitPacket::build(&[1, 2, 3], &t1000e_spec()?)?;
        assert_eq!(
            packet.bytes(),
            &[0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad]
        );
        Ok(())
    }

    #[test]
    fn softdevice_requirements_are_canonical() -> Result<(), DfuImageError> {
        let s140_v6 = SoftdeviceFirmwareId::new(0x00b6)?;
        let s140_v7 = SoftdeviceFirmwareId::new(0x0123)?;
        let requirements = SoftdeviceRequirements::new(s140_v7, [s140_v6, s140_v7])?;
        assert_eq!(requirements.as_slice(), &[s140_v6, s140_v7]);
        Ok(())
    }

    #[test]
    fn image_rejects_an_init_packet_for_different_firmware() -> Result<(), DfuImageError> {
        let packet = ApplicationInitPacket::build(&[1, 2, 3], &t1000e_spec()?)?;
        assert_eq!(
            DfuImage::new(&[1, 2, 4], packet),
            Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: 0xadad,
                firmware_crc: 0xdd4a,
            })
        );
        Ok(())
    }

    #[test]
    fn image_accepts_matching_external_artifacts() -> Result<(), DfuImageError> {
        let firmware = [1, 2, 3];
        let init_packet = [
            0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
        ];
        let image = DfuImage::from_artifacts(&firmware, &init_packet, &t1000e_spec()?)?;
        assert_eq!(image.firmware(), firmware);
        assert_eq!(image.init_packet().bytes(), init_packet);
        Ok(())
    }

    #[test]
    fn owned_image_retains_validated_artifacts() -> Result<(), DfuImageError> {
        let firmware = vec![1, 2, 3];
        let init_packet = vec![
            0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
        ];
        let image = DfuImage::from_owned_artifacts(firmware, init_packet, &t1000e_spec()?)?;

        assert_eq!(image.firmware(), &[1, 2, 3]);
        assert_eq!(
            image.init_packet().bytes(),
            &[0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad]
        );
        Ok(())
    }

    #[test]
    fn owned_image_rejects_an_init_packet_for_different_firmware() -> Result<(), DfuImageError> {
        let firmware = vec![1, 2, 4];
        let init_packet = vec![
            0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
        ];

        assert_eq!(
            DfuImage::from_owned_artifacts(firmware, init_packet, &t1000e_spec()?),
            Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: 0xadad,
                firmware_crc: 0xdd4a,
            })
        );
        Ok(())
    }

    #[test]
    fn external_init_packet_must_match_the_device_specification() -> Result<(), DfuImageError> {
        let firmware = [1, 2, 3];
        let mut init_packet = [
            0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
        ];
        init_packet[0] = 0x53;
        assert_eq!(
            DfuImage::from_artifacts(&firmware, &init_packet, &t1000e_spec()?),
            Err(DfuImageError::InitPacketMetadataMismatch {
                offset: 0,
                expected: 0x52,
                actual: 0x53,
            })
        );
        Ok(())
    }

    #[test]
    fn external_init_packet_must_match_the_firmware() -> Result<(), DfuImageError> {
        let firmware = [1, 2, 4];
        let init_packet = [
            0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
        ];
        assert_eq!(
            DfuImage::from_artifacts(&firmware, &init_packet, &t1000e_spec()?),
            Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: 0xadad,
                firmware_crc: 0xdd4a,
            })
        );
        Ok(())
    }

    #[test]
    fn softdevice_wildcard_is_not_an_exact_firmware_identity() {
        assert_eq!(
            SoftdeviceFirmwareId::new(0xfffe),
            Err(DfuImageError::WildcardSoftdeviceRequirement)
        );
    }
}
