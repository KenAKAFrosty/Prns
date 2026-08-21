use std::collections::BTreeSet;

use thiserror::Error;

use crate::{NrfSerialDfuTarget, SoftdeviceIdentity, Uf2BoardIdMatch, Uf2Variant};

const MAX_INFO_UF2_BYTES: usize = 4096;
const MAX_INFO_UF2_LINE_BYTES: usize = 512;
const UF2_BLOCK_BYTES: usize = 512;
const UF2_DATA_OFFSET: usize = 32;
const UF2_DATA_BYTES: usize = 476;
const UF2_PAYLOAD_BYTES: u32 = 256;
const UF2_MAGIC_START_ZERO: u32 = 0x0a32_4655;
const UF2_MAGIC_START_ONE: u32 = 0x9e5d_5157;
const UF2_MAGIC_END: u32 = 0x0ab1_6f30;
const UF2_FAMILY_ID_FLAG: u32 = 0x0000_2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uf2BootloaderIdentity {
    board_id: String,
    bootloader_version: String,
    softdevice: SoftdeviceIdentity,
}

impl Uf2BootloaderIdentity {
    pub fn parse(bytes: &[u8]) -> Result<Self, Uf2IdentityError> {
        if bytes.len() > MAX_INFO_UF2_BYTES {
            return Err(Uf2IdentityError::Oversized(bytes.len()));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Uf2IdentityError::Encoding)?;
        let mut fields = BTreeSet::new();
        let mut board_id = None;
        let mut bootloader_version = None;
        let mut softdevice = None;
        for line in text.lines() {
            if line.len() > MAX_INFO_UF2_LINE_BYTES {
                return Err(Uf2IdentityError::LineOversized(line.len()));
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(value) = line.strip_prefix("UF2 Bootloader ") {
                if !fields.insert("uf2bootloader".to_string()) {
                    return Err(Uf2IdentityError::DuplicateField(
                        "UF2 Bootloader".to_string(),
                    ));
                }
                let value = value
                    .split_ascii_whitespace()
                    .next()
                    .ok_or(Uf2IdentityError::BootloaderVersion)?;
                if !valid_bootloader_version(value) {
                    return Err(Uf2IdentityError::BootloaderVersion);
                }
                bootloader_version = Some(value.to_string());
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(Uf2IdentityError::MalformedLine(line.to_string()));
            };
            let normalized_name = name
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .map(|character| character.to_ascii_lowercase())
                .collect::<String>();
            if normalized_name.is_empty() {
                return Err(Uf2IdentityError::MalformedLine(line.to_string()));
            }
            if !fields.insert(normalized_name.clone()) {
                return Err(Uf2IdentityError::DuplicateField(name.trim().to_string()));
            }
            match normalized_name.as_str() {
                "boardid" => {
                    let normalized = Uf2BoardIdMatch::normalize(value);
                    let valid = !normalized.is_empty()
                        && normalized.len() <= 128
                        && normalized.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'.' | b'-')
                        });
                    if !valid {
                        return Err(Uf2IdentityError::BoardId);
                    }
                    board_id = Some(normalized);
                }
                "softdevice" => {
                    let values = value.split_ascii_whitespace().collect::<Vec<_>>();
                    let (family, version) = match values.as_slice() {
                        [family, marker, version] if marker.eq_ignore_ascii_case("version") => {
                            (family, version)
                        }
                        [family, version] => (family, version),
                        _ => return Err(Uf2IdentityError::Softdevice),
                    };
                    softdevice = Some(
                        SoftdeviceIdentity::parse(family, (*version).to_string())
                            .map_err(|_| Uf2IdentityError::Softdevice)?,
                    );
                }
                _ => {}
            }
        }
        Ok(Self {
            board_id: board_id.ok_or(Uf2IdentityError::MissingField("Board-ID"))?,
            bootloader_version: bootloader_version
                .ok_or(Uf2IdentityError::MissingField("UF2 Bootloader"))?,
            softdevice: softdevice.ok_or(Uf2IdentityError::MissingField("SoftDevice"))?,
        })
    }

    pub fn board_id(&self) -> &str {
        &self.board_id
    }

    pub fn bootloader_version(&self) -> &str {
        &self.bootloader_version
    }

    pub fn softdevice(&self) -> &SoftdeviceIdentity {
        &self.softdevice
    }

    pub fn matches_board(&self, board_id_match: &Uf2BoardIdMatch) -> bool {
        board_id_match.matches(&self.board_id)
    }
}

fn valid_bootloader_version(value: &str) -> bool {
    let core_end = value.find(['-', '+']).unwrap_or(value.len());
    let core = &value[..core_end];
    let components = core.split('.').collect::<Vec<_>>();
    let core_is_valid = components.len() == 3
        && components.iter().all(|component| {
            !component.is_empty()
                && component.bytes().all(|byte| byte.is_ascii_digit())
                && (component == &"0" || !component.starts_with('0'))
                && component.parse::<u16>().is_ok()
        });
    if !core_is_valid || core_end == value.len() {
        return core_is_valid;
    }
    value[core_end + 1..]
        .split(['.', '-', '+'])
        .all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Uf2IdentityError {
    #[error("INFO_UF2.TXT is {0} bytes; the maximum is 4096")]
    Oversized(usize),
    #[error("INFO_UF2.TXT is not valid UTF-8")]
    Encoding,
    #[error("INFO_UF2.TXT contains a {0}-byte line; the maximum is 512")]
    LineOversized(usize),
    #[error("INFO_UF2.TXT repeats field {0:?}")]
    DuplicateField(String),
    #[error("INFO_UF2.TXT contains malformed line {0:?}")]
    MalformedLine(String),
    #[error("INFO_UF2.TXT has no valid UF2 bootloader version")]
    BootloaderVersion,
    #[error("INFO_UF2.TXT has no valid Board-ID")]
    BoardId,
    #[error("INFO_UF2.TXT has no valid SoftDevice identity")]
    Softdevice,
    #[error("INFO_UF2.TXT is missing required field {0}")]
    MissingField(&'static str),
}

pub fn validate_uf2_artifact(variant: &Uf2Variant, bytes: &[u8]) -> Result<(), Uf2ArtifactError> {
    validate_uf2_bytes(
        variant.compatibility().application_base(),
        variant.compatibility().application_end_exclusive(),
        variant.compatibility().family_id(),
        bytes,
    )
}

pub fn validate_nrf_serial_dfu_recovery_artifact(
    target: &NrfSerialDfuTarget,
    application: &[u8],
    recovery: &[u8],
) -> Result<(), Uf2ArtifactError> {
    validate_uf2_bytes(
        target.compatibility().application_base(),
        target.compatibility().application_end_exclusive(),
        target.recovery().family_id(),
        recovery,
    )?;
    validate_recovery_application(application, recovery)
}

fn validate_recovery_application(
    application: &[u8],
    recovery: &[u8],
) -> Result<(), Uf2ArtifactError> {
    let expected_blocks = application.len().div_ceil(UF2_PAYLOAD_BYTES as usize);
    let actual_blocks = recovery.len() / UF2_BLOCK_BYTES;
    if application.is_empty() || actual_blocks != expected_blocks {
        return Err(Uf2ArtifactError::ApplicationLength {
            application_bytes: application.len(),
            uf2_blocks: actual_blocks,
        });
    }
    for (index, block) in recovery.as_chunks::<UF2_BLOCK_BYTES>().0.iter().enumerate() {
        let application_offset = index * UF2_PAYLOAD_BYTES as usize;
        let application_end =
            (application_offset + UF2_PAYLOAD_BYTES as usize).min(application.len());
        let expected = &application[application_offset..application_end];
        let payload = &block[UF2_DATA_OFFSET..UF2_DATA_OFFSET + UF2_PAYLOAD_BYTES as usize];
        if payload[..expected.len()] != *expected
            || payload[expected.len()..].iter().any(|byte| *byte != 0)
        {
            return Err(Uf2ArtifactError::ApplicationData(index as u32));
        }
    }
    Ok(())
}

fn validate_uf2_bytes(
    application_base: u32,
    application_end_exclusive: u32,
    family_id: u32,
    bytes: &[u8],
) -> Result<(), Uf2ArtifactError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(UF2_BLOCK_BYTES) {
        return Err(Uf2ArtifactError::Length(bytes.len()));
    }
    let block_count = bytes.len() / UF2_BLOCK_BYTES;
    let declared_blocks =
        u32::try_from(block_count).map_err(|_| Uf2ArtifactError::Length(bytes.len()))?;
    let mut expected_address = application_base;
    for (index, block) in bytes.as_chunks::<UF2_BLOCK_BYTES>().0.iter().enumerate() {
        let block_number =
            u32::try_from(index).map_err(|_| Uf2ArtifactError::Length(bytes.len()))?;
        if word(block, 0) != UF2_MAGIC_START_ZERO
            || word(block, 4) != UF2_MAGIC_START_ONE
            || word(block, 508) != UF2_MAGIC_END
        {
            return Err(Uf2ArtifactError::Magic(block_number));
        }
        if word(block, 8) != UF2_FAMILY_ID_FLAG {
            return Err(Uf2ArtifactError::Flags(block_number));
        }
        if word(block, 20) != block_number || word(block, 24) != declared_blocks {
            return Err(Uf2ArtifactError::Order(block_number));
        }
        if word(block, 28) != family_id {
            return Err(Uf2ArtifactError::Family(block_number));
        }
        let address = word(block, 12);
        let payload = word(block, 16);
        if address != expected_address {
            return Err(Uf2ArtifactError::Address(block_number));
        }
        if payload != UF2_PAYLOAD_BYTES || payload as usize > UF2_DATA_BYTES {
            return Err(Uf2ArtifactError::Payload(block_number));
        }
        let end = address
            .checked_add(payload)
            .ok_or(Uf2ArtifactError::Bounds(block_number))?;
        if end > application_end_exclusive {
            return Err(Uf2ArtifactError::Bounds(block_number));
        }
        expected_address = end;
        if block[UF2_DATA_OFFSET + payload as usize..UF2_DATA_OFFSET + UF2_DATA_BYTES]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(Uf2ArtifactError::Padding(block_number));
        }
    }
    Ok(())
}

fn word(block: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        block[offset],
        block[offset + 1],
        block[offset + 2],
        block[offset + 3],
    ])
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Uf2ArtifactError {
    #[error("UF2 length {0} is not a nonzero multiple of 512 bytes")]
    Length(usize),
    #[error("UF2 block {0} has invalid magic")]
    Magic(u32),
    #[error("UF2 block {0} has unsupported flags")]
    Flags(u32),
    #[error("UF2 block {0} is reordered or declares the wrong block count")]
    Order(u32),
    #[error("UF2 block {0} has the wrong family ID")]
    Family(u32),
    #[error("UF2 block {0} is not at the next contiguous application address")]
    Address(u32),
    #[error("UF2 block {0} has an unsupported payload length")]
    Payload(u32),
    #[error("UF2 block {0} exceeds the application flash region")]
    Bounds(u32),
    #[error("UF2 block {0} has nonzero bytes outside its payload")]
    Padding(u32),
    #[error("UF2 recovery has {uf2_blocks} blocks for a {application_bytes}-byte application")]
    ApplicationLength {
        application_bytes: usize,
        uf2_blocks: usize,
    },
    #[error("UF2 block {0} does not contain the exact application bytes")]
    ApplicationData(u32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImmutableArtifactPath, Sha256Digest, Uf2Compatibility, Uf2Part};

    fn info(line_ending: &str, board_id: &str, version: &str) -> Vec<u8> {
        [
            "UF2 Bootloader 0.6.1-2-g1224915",
            "Model: LilyGo T-Echo",
            &format!("Board-ID: {board_id}"),
            &format!("SoftDevice: S140 version {version}"),
            "Date: Oct 13 2021",
        ]
        .join(line_ending)
        .into_bytes()
    }

    fn variant(version: &str, application_base: u32, fwid: u16) -> Uf2Variant {
        Uf2Variant {
            compatibility: Uf2Compatibility::new(
                SoftdeviceIdentity::parse("s140", version.to_string())
                    .expect("SoftDevice identity"),
                fwid,
                application_base,
                0x000c_0000,
                0xada5_2840,
            ),
            part: Uf2Part {
                path: ImmutableArtifactPath::parse(format!("t-echo-{version}.uf2"))
                    .expect("artifact path"),
                size: 512,
                sha256: Sha256Digest::parse("a".repeat(64)).expect("digest"),
            },
        }
    }

    fn artifact(application_base: u32, family_id: u32, blocks: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for index in 0..blocks {
            let mut block = [0_u8; UF2_BLOCK_BYTES];
            for (offset, value) in [
                (0, UF2_MAGIC_START_ZERO),
                (4, UF2_MAGIC_START_ONE),
                (8, UF2_FAMILY_ID_FLAG),
                (12, application_base + index * UF2_PAYLOAD_BYTES),
                (16, UF2_PAYLOAD_BYTES),
                (20, index),
                (24, blocks),
                (28, family_id),
                (508, UF2_MAGIC_END),
            ] {
                block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&block);
        }
        bytes
    }

    #[test]
    fn descriptors_accept_lf_crlf_and_normalized_board_ids() {
        let board_id_match = Uf2BoardIdMatch::parse(
            crate::Uf2BoardIdMatchKind::RevisionPrefix,
            "nrf52840-techo-v",
        )
        .expect("match rule");
        for (line_ending, board_id, version) in [
            ("\n", "nRF52840-TEcho-v1", "6.1.1"),
            ("\r\n", "nRF52840_TEcho_v2.1", "7.3.0"),
        ] {
            let identity = Uf2BootloaderIdentity::parse(&info(line_ending, board_id, version))
                .expect("valid descriptor");
            assert!(identity.matches_board(&board_id_match));
            assert_eq!(identity.softdevice().version().as_str(), version);
            assert_eq!(identity.bootloader_version(), "0.6.1-2-g1224915");
        }
        let board_id_match =
            Uf2BoardIdMatch::parse(crate::Uf2BoardIdMatchKind::Exact, "nrf52840-t1000-e-v1")
                .expect("match rule");
        let identity = Uf2BootloaderIdentity::parse(&info("\n", "nRF52840-T1000-E-v1", "7.3.0"))
            .expect("valid descriptor");
        assert!(identity.matches_board(&board_id_match));
    }

    #[test]
    fn exact_heltec_board_ids_do_not_cross_match() {
        let bytes = [
            "UF2 Bootloader 0.9.0-2-g836c8dc-dirty",
            "Model: HT-n5262",
            "Board-ID: HT-n5262",
            "Date: Jul  9 2024",
            "SoftDevice: S140 6.1.1",
        ]
        .join("\n")
        .into_bytes();
        let t114_identity = Uf2BootloaderIdentity::parse(&bytes).expect("valid descriptor");
        let t096_identity = Uf2BootloaderIdentity::parse(&info("\n", "HT-n5262G", "6.1.1"))
            .expect("valid descriptor");
        let t114_match = Uf2BoardIdMatch::parse(crate::Uf2BoardIdMatchKind::Exact, "ht-n5262")
            .expect("T114 match rule");
        let t096_match = Uf2BoardIdMatch::parse(crate::Uf2BoardIdMatchKind::Exact, "ht-n5262g")
            .expect("T096 match rule");

        assert!(t114_identity.matches_board(&t114_match));
        assert!(!t114_identity.matches_board(&t096_match));
        assert!(t096_identity.matches_board(&t096_match));
        assert!(!t096_identity.matches_board(&t114_match));
        assert_eq!(t114_identity.bootloader_version(), "0.9.0-2-g836c8dc-dirty");
        assert_eq!(t114_identity.softdevice().family().as_str(), "s140");
        assert_eq!(t114_identity.softdevice().version().as_str(), "6.1.1");
    }

    #[test]
    fn descriptors_reject_missing_duplicate_oversized_and_malformed_identity() {
        for bytes in [
            b"Board-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 0.6.1\nBoard-ID: nRF52840-TEcho-v1\nBoard ID: nRF52840-TEcho-v2\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader broken!\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 1..2\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 1.2\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 1.2.3-\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 0.6.1\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S132 version 7.3.0\n".to_vec(),
            b"UF2 Bootloader 0.6.1\nnot a field\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n".to_vec(),
            vec![b'x'; MAX_INFO_UF2_BYTES + 1],
            [
                b"UF2 Bootloader 0.6.1\nBoard-ID: ".as_slice(),
                vec![b'x'; MAX_INFO_UF2_LINE_BYTES + 1].as_slice(),
            ]
            .concat(),
            vec![0xff],
        ] {
            assert!(Uf2BootloaderIdentity::parse(&bytes).is_err());
        }
    }

    #[test]
    fn uf2_artifacts_require_order_base_family_bounds_and_exact_compatibility() {
        let v6 = variant("6.1.1", 0x26000, 0x00b6);
        let v7 = variant("7.3.0", 0x27000, 0x0123);
        let v6_bytes = artifact(0x26000, 0xada5_2840, 2);
        let v7_bytes = artifact(0x27000, 0xada5_2840, 2);
        assert_eq!(validate_uf2_artifact(&v6, &v6_bytes), Ok(()));
        assert_eq!(validate_uf2_artifact(&v7, &v7_bytes), Ok(()));
        assert!(matches!(
            validate_uf2_artifact(&v7, &v6_bytes),
            Err(Uf2ArtifactError::Address(0))
        ));

        let mut corrupt = v7_bytes.clone();
        corrupt[0] = 0;
        assert!(matches!(
            validate_uf2_artifact(&v7, &corrupt),
            Err(Uf2ArtifactError::Magic(0))
        ));

        let mut reordered = v7_bytes.clone();
        reordered[20..24].copy_from_slice(&1_u32.to_le_bytes());
        assert!(matches!(
            validate_uf2_artifact(&v7, &reordered),
            Err(Uf2ArtifactError::Order(0))
        ));

        let wrong_base = artifact(0x27100, 0xada5_2840, 1);
        assert!(matches!(
            validate_uf2_artifact(&v7, &wrong_base),
            Err(Uf2ArtifactError::Address(0))
        ));

        let wrong_family = artifact(0x27000, 0x1234_5678, 1);
        assert!(matches!(
            validate_uf2_artifact(&v7, &wrong_family),
            Err(Uf2ArtifactError::Family(0))
        ));

        let mut padding = artifact(0x27000, 0xada5_2840, 1);
        padding[UF2_DATA_OFFSET + UF2_PAYLOAD_BYTES as usize] = 1;
        assert!(matches!(
            validate_uf2_artifact(&v7, &padding),
            Err(Uf2ArtifactError::Padding(0))
        ));
    }

    #[test]
    fn recovery_uf2_contains_the_exact_application() {
        let application = (0..300).map(|value| value as u8).collect::<Vec<_>>();
        let mut recovery = artifact(0x27000, 0xada5_2840, 2);
        recovery[UF2_DATA_OFFSET..UF2_DATA_OFFSET + 256].copy_from_slice(&application[..256]);
        recovery[UF2_BLOCK_BYTES + UF2_DATA_OFFSET
            ..UF2_BLOCK_BYTES + UF2_DATA_OFFSET + application.len() - 256]
            .copy_from_slice(&application[256..]);
        assert_eq!(
            validate_recovery_application(&application, &recovery),
            Ok(())
        );

        let mut altered = recovery.clone();
        altered[UF2_BLOCK_BYTES + UF2_DATA_OFFSET] ^= 1;
        assert_eq!(
            validate_recovery_application(&application, &altered),
            Err(Uf2ArtifactError::ApplicationData(1))
        );

        let mut padded = recovery;
        padded[UF2_BLOCK_BYTES + UF2_DATA_OFFSET + application.len() - 256] = 1;
        assert_eq!(
            validate_recovery_application(&application, &padded),
            Err(Uf2ArtifactError::ApplicationData(1))
        );
    }
}
