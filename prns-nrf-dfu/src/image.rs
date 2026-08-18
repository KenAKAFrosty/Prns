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

impl ApplicationInitPacket {
    pub fn build(
        firmware: &[u8],
        device_type: DfuDeviceType,
        device_revision: DfuDeviceRevision,
        application_version: ApplicationVersion,
        softdevices: &SoftdeviceRequirements,
    ) -> Result<Self, DfuImageError> {
        if firmware.is_empty() {
            return Err(DfuImageError::EmptyFirmware);
        }
        if firmware.len() > u32::MAX as usize {
            return Err(DfuImageError::FirmwareTooLarge {
                actual: firmware.len(),
                maximum: u32::MAX as usize,
            });
        }

        let mut bytes = Vec::with_capacity(12 + softdevices.values.len() * 2);
        bytes.extend_from_slice(&device_type.0.to_le_bytes());
        bytes.extend_from_slice(&device_revision.0.to_le_bytes());
        bytes.extend_from_slice(&application_version.wire_value().to_le_bytes());
        bytes.extend_from_slice(&(softdevices.values.len() as u16).to_le_bytes());
        for softdevice in &softdevices.values {
            bytes.extend_from_slice(&softdevice.0.to_le_bytes());
        }
        let firmware_crc = firmware_crc(firmware);
        bytes.extend_from_slice(&firmware_crc.get().to_le_bytes());
        Ok(Self {
            bytes,
            firmware_crc,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfuImage<'a> {
    firmware: &'a [u8],
    init_packet: ApplicationInitPacket,
}

impl<'a> DfuImage<'a> {
    pub fn new(
        firmware: &'a [u8],
        init_packet: ApplicationInitPacket,
    ) -> Result<Self, DfuImageError> {
        if firmware.is_empty() {
            return Err(DfuImageError::EmptyFirmware);
        }
        if firmware.len() > u32::MAX as usize {
            return Err(DfuImageError::FirmwareTooLarge {
                actual: firmware.len(),
                maximum: u32::MAX as usize,
            });
        }
        let actual_firmware_crc = firmware_crc(firmware);
        if init_packet.firmware_crc != actual_firmware_crc {
            return Err(DfuImageError::InitPacketFirmwareMismatch {
                init_packet_crc: init_packet.firmware_crc.get(),
                firmware_crc: actual_firmware_crc.get(),
            });
        }
        Ok(Self {
            firmware,
            init_packet,
        })
    }

    pub fn firmware(&self) -> &[u8] {
        self.firmware
    }

    pub fn init_packet(&self) -> &ApplicationInitPacket {
        &self.init_packet
    }
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
    use super::{
        ApplicationInitPacket, ApplicationVersion, DfuDeviceRevision, DfuDeviceType, DfuImage,
        DfuImageError, SoftdeviceFirmwareId, SoftdeviceRequirements,
    };

    #[test]
    fn init_packet_matches_adafruit_nrfutil_reference() -> Result<(), DfuImageError> {
        let fwid = SoftdeviceFirmwareId::new(0x0123)?;
        let requirements = SoftdeviceRequirements::new(fwid, std::iter::empty())?;
        let packet = ApplicationInitPacket::build(
            &[1, 2, 3],
            DfuDeviceType::new(0x0052),
            DfuDeviceRevision::new(52840),
            ApplicationVersion::NotEnforced,
            &requirements,
        )?;
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
        let fwid = SoftdeviceFirmwareId::new(0x0123)?;
        let requirements = SoftdeviceRequirements::new(fwid, std::iter::empty())?;
        let packet = ApplicationInitPacket::build(
            &[1, 2, 3],
            DfuDeviceType::new(0x0052),
            DfuDeviceRevision::new(52840),
            ApplicationVersion::NotEnforced,
            &requirements,
        )?;
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
    fn softdevice_wildcard_is_not_an_exact_firmware_identity() {
        assert_eq!(
            SoftdeviceFirmwareId::new(0xfffe),
            Err(DfuImageError::WildcardSoftdeviceRequirement)
        );
    }
}
