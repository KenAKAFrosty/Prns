use crate::routing::announce::emit::{AnnounceAppDataBytes, MAX_ANNOUNCE_APP_DATA_LEN};

const ANNOUNCEMENT_FORMAT_VERSION_ENCODED_LEN: usize = 1;

pub const MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN: usize =
    MAX_ANNOUNCE_APP_DATA_LEN.saturating_sub(ANNOUNCEMENT_FORMAT_VERSION_ENCODED_LEN);

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlAnnouncementFormatVersion {
        V1 = 1,
    }
}

impl RemoteControlAnnouncementFormatVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|version| version.wire_value() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPublicAppDataError {
    TooLong { actual: usize, maximum: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPublicAppData<'a> {
    bytes: &'a [u8],
}

impl<'a> RemoteControlPublicAppData<'a> {
    #[must_use]
    pub const fn empty() -> Self {
        Self { bytes: &[] }
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl<'a> TryFrom<&'a [u8]> for RemoteControlPublicAppData<'a> {
    type Error = RemoteControlPublicAppDataError;

    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        if bytes.len() > MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN {
            return Err(RemoteControlPublicAppDataError::TooLong {
                actual: bytes.len(),
                maximum: MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN,
            });
        }
        Ok(Self { bytes })
    }
}

impl AsRef<[u8]> for RemoteControlPublicAppData<'_> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlAnnouncementData<'a> {
    format_version: RemoteControlAnnouncementFormatVersion,
    public_app_data: RemoteControlPublicAppData<'a>,
}

impl<'a> RemoteControlAnnouncementData<'a> {
    pub const MAX_ENCODED_LEN: usize = MAX_ANNOUNCE_APP_DATA_LEN;

    #[must_use]
    pub const fn new(public_app_data: RemoteControlPublicAppData<'a>) -> Self {
        Self {
            format_version: RemoteControlAnnouncementFormatVersion::V1,
            public_app_data,
        }
    }

    #[must_use]
    pub const fn format_version(&self) -> RemoteControlAnnouncementFormatVersion {
        self.format_version
    }

    #[must_use]
    pub const fn public_app_data(&self) -> &RemoteControlPublicAppData<'a> {
        &self.public_app_data
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        ANNOUNCEMENT_FORMAT_VERSION_ENCODED_LEN.saturating_add(self.public_app_data.len())
    }

    pub fn write_into(
        &self,
        output: &mut [u8],
    ) -> Result<usize, RemoteControlAnnouncementDataWriteError> {
        let encoded_len = self.encoded_len();
        let Some(encoded) = output.get_mut(..encoded_len) else {
            return Err(RemoteControlAnnouncementDataWriteError::BufferTooShort);
        };
        let Some((format_version, public_app_data)) = encoded.split_first_mut() else {
            return Err(RemoteControlAnnouncementDataWriteError::BufferTooShort);
        };

        *format_version = self.format_version.wire_value();
        public_app_data.copy_from_slice(self.public_app_data.as_bytes());
        Ok(encoded_len)
    }

    pub fn encode(&self) -> Result<AnnounceAppDataBytes, RemoteControlAnnouncementDataWriteError> {
        let mut output = [0; Self::MAX_ENCODED_LEN];
        let encoded_len = self.write_into(&mut output)?;
        let encoded = output
            .get(..encoded_len)
            .ok_or(RemoteControlAnnouncementDataWriteError::BufferTooShort)?;
        AnnounceAppDataBytes::from_slice(encoded)
            .map_err(|()| RemoteControlAnnouncementDataWriteError::BufferTooShort)
    }

    pub fn parse(bytes: &'a [u8]) -> Result<Self, RemoteControlAnnouncementDataParseError> {
        let Some((format_version, public_app_data)) = bytes.split_first() else {
            return Err(RemoteControlAnnouncementDataParseError::Truncated);
        };
        let Some(format_version) =
            RemoteControlAnnouncementFormatVersion::from_wire(*format_version)
        else {
            return Err(
                RemoteControlAnnouncementDataParseError::UnsupportedFormatVersion {
                    found: *format_version,
                },
            );
        };

        Ok(Self {
            format_version,
            public_app_data: RemoteControlPublicAppData::try_from(public_app_data)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAnnouncementDataWriteError {
    BufferTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAnnouncementDataParseError {
    Truncated,
    UnsupportedFormatVersion { found: u8 },
    PublicAppData(RemoteControlPublicAppDataError),
}

impl From<RemoteControlPublicAppDataError> for RemoteControlAnnouncementDataParseError {
    fn from(error: RemoteControlPublicAppDataError) -> Self {
        Self::PublicAppData(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERSIZED_PUBLIC_APP_DATA_LEN: usize =
        MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN.saturating_add(1);

    fn announcement_data(
        public_app_data: &[u8],
    ) -> Result<RemoteControlAnnouncementData<'_>, RemoteControlPublicAppDataError> {
        Ok(RemoteControlAnnouncementData::new(
            RemoteControlPublicAppData::try_from(public_app_data)?,
        ))
    }

    #[test]
    fn public_app_data_uses_every_byte_left_by_the_format_version() {
        let maximum = [0xA5; MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN];
        let oversized = [0x5A; OVERSIZED_PUBLIC_APP_DATA_LEN];

        assert!(RemoteControlPublicAppData::empty().as_bytes().is_empty());
        assert_eq!(
            RemoteControlAnnouncementData::MAX_ENCODED_LEN,
            MAX_ANNOUNCE_APP_DATA_LEN,
        );
        assert_eq!(
            RemoteControlPublicAppData::try_from(maximum.as_slice())
                .as_ref()
                .map(|data| data.as_bytes()),
            Ok(maximum.as_slice()),
        );
        assert_eq!(
            RemoteControlPublicAppData::try_from(oversized.as_slice()),
            Err(RemoteControlPublicAppDataError::TooLong {
                actual: oversized.len(),
                maximum: MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN,
            }),
        );
    }

    #[test]
    fn announcement_data_round_trips_only_the_version_and_public_app_data(
    ) -> Result<(), RemoteControlPublicAppDataError> {
        let announcement_data = announcement_data(b"PRNS")?;
        let encoded_len = announcement_data.encoded_len();
        let mut output = [0; RemoteControlAnnouncementData::MAX_ENCODED_LEN];
        let expected = [
            RemoteControlAnnouncementFormatVersion::V1.wire_value(),
            b'P',
            b'R',
            b'N',
            b'S',
        ];

        assert_eq!(announcement_data.write_into(&mut output), Ok(encoded_len));
        assert_eq!(
            announcement_data
                .encode()
                .as_ref()
                .map(|data| data.as_slice()),
            Ok(expected.as_slice()),
        );
        assert_eq!(output.get(..encoded_len), Some(expected.as_slice()));
        assert_eq!(
            output
                .get(..encoded_len)
                .map(RemoteControlAnnouncementData::parse),
            Some(Ok(announcement_data)),
        );

        Ok(())
    }

    #[test]
    fn empty_public_app_data_encodes_as_only_the_format_version(
    ) -> Result<(), RemoteControlPublicAppDataError> {
        let announcement_data = announcement_data(b"")?;
        let mut output = [0xA5; RemoteControlAnnouncementData::MAX_ENCODED_LEN];
        let mut expected = output;
        assert_eq!(
            expected.first_mut().map(|format_version| {
                *format_version = RemoteControlAnnouncementFormatVersion::V1.wire_value()
            }),
            Some(()),
        );

        assert_eq!(announcement_data.write_into(&mut output), Ok(1));
        assert_eq!(output, expected);

        Ok(())
    }

    #[test]
    fn announcement_data_writer_refuses_every_short_buffer(
    ) -> Result<(), RemoteControlPublicAppDataError> {
        let announcement_data = announcement_data(b"application")?;
        let mut output = [0; RemoteControlAnnouncementData::MAX_ENCODED_LEN];

        for short_len in 0..announcement_data.encoded_len() {
            assert_eq!(
                output
                    .get_mut(..short_len)
                    .map(|short| announcement_data.write_into(short)),
                Some(Err(RemoteControlAnnouncementDataWriteError::BufferTooShort)),
            );
        }

        Ok(())
    }

    #[test]
    fn announcement_data_parser_preserves_each_failure_class() {
        assert_eq!(
            RemoteControlAnnouncementData::parse(&[]),
            Err(RemoteControlAnnouncementDataParseError::Truncated),
        );
        assert_eq!(
            RemoteControlAnnouncementData::parse(&[2]),
            Err(RemoteControlAnnouncementDataParseError::UnsupportedFormatVersion { found: 2 }),
        );

        let mut oversized = [0x5A; MAX_ANNOUNCE_APP_DATA_LEN.saturating_add(1)];
        assert_eq!(
            oversized.first_mut().map(|version| {
                *version = RemoteControlAnnouncementFormatVersion::V1.wire_value()
            }),
            Some(()),
        );
        assert_eq!(
            RemoteControlAnnouncementData::parse(&oversized),
            Err(RemoteControlAnnouncementDataParseError::PublicAppData(
                RemoteControlPublicAppDataError::TooLong {
                    actual: OVERSIZED_PUBLIC_APP_DATA_LEN,
                    maximum: MAX_REMOTE_CONTROL_PUBLIC_APP_DATA_LEN,
                },
            )),
        );
    }
}
