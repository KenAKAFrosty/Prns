use crate::crypto::sha256;

/// Stable supervisor channel tag for [`InterfaceId::from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag).
/// This is not a discovery/peering group — see [`GROUP_ID`].
pub const CHANNEL_TAG: &[u8] = b"bluetooth-auto";

/// Default Bluetooth Auto discovery group (same string as Wi‑Fi Auto's default).
pub const GROUP_NAME: &str = "reticulum";
/// UTF-8 bytes of [`GROUP_NAME`], hashed into the advertisement group tag.
pub const GROUP_ID: &[u8] = GROUP_NAME.as_bytes();

pub const GROUP_TAG_LEN: usize = 4;

pub const BLE_IDENTITY_LEN: usize = 16;
pub const PERSISTED_BLE_IDENTITY_LEN: usize = 40;

const PERSISTED_BLE_IDENTITY_MAGIC: [u8; 8] = *b"PRNSBLE1";

/// Truncated `sha256(group_id)` carried in manufacturer-specific advertisement data.
pub fn group_tag(group_id: &[u8]) -> [u8; GROUP_TAG_LEN] {
    let digest = sha256(group_id);
    let mut tag = [0u8; GROUP_TAG_LEN];
    tag.copy_from_slice(&digest[..GROUP_TAG_LEN]);
    tag
}

/// Group tag for the default discovery group [`GROUP_ID`].
pub fn default_group_tag() -> [u8; GROUP_TAG_LEN] {
    group_tag(GROUP_ID)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleAddress([u8; 6]);

impl BleAddress {
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    pub const fn from_hci_bytes(bytes: [u8; 6]) -> Self {
        Self([bytes[5], bytes[4], bytes[3], bytes[2], bytes[1], bytes[0]])
    }

    pub const fn octets(&self) -> &[u8; 6] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleIdentity([u8; BLE_IDENTITY_LEN]);

impl BleIdentity {
    pub const fn new(bytes: [u8; BLE_IDENTITY_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; BLE_IDENTITY_LEN] {
        &self.0
    }
}

pub fn encode_persisted_ble_identity(identity: BleIdentity) -> [u8; PERSISTED_BLE_IDENTITY_LEN] {
    let mut record = [0u8; PERSISTED_BLE_IDENTITY_LEN];
    record[..PERSISTED_BLE_IDENTITY_MAGIC.len()].copy_from_slice(&PERSISTED_BLE_IDENTITY_MAGIC);
    record[8..24].copy_from_slice(identity.as_bytes());
    for (encoded, byte) in record[24..].iter_mut().zip(identity.as_bytes()) {
        *encoded = !byte;
    }
    record
}

pub fn decode_persisted_ble_identity(
    record: &[u8; PERSISTED_BLE_IDENTITY_LEN],
) -> Result<Option<BleIdentity>, PersistedBleIdentityError> {
    if record.iter().all(|byte| *byte == u8::MAX) {
        return Ok(None);
    }
    if record[..PERSISTED_BLE_IDENTITY_MAGIC.len()] != PERSISTED_BLE_IDENTITY_MAGIC {
        return Err(PersistedBleIdentityError::Magic);
    }
    let mut identity = [0u8; BLE_IDENTITY_LEN];
    identity.copy_from_slice(&record[8..24]);
    if record[24..]
        .iter()
        .zip(identity)
        .any(|(encoded, byte)| *encoded != !byte)
    {
        return Err(PersistedBleIdentityError::Integrity);
    }
    Ok(Some(BleIdentity::new(identity)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedBleIdentityError {
    Magic,
    Integrity,
}

impl core::fmt::Display for PersistedBleIdentityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Magic => formatter.write_str("persisted BLE identity has invalid magic"),
            Self::Integrity => {
                formatter.write_str("persisted BLE identity failed integrity validation")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PersistedBleIdentityError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_identity_record_round_trips_and_detects_partial_writes() {
        let identity = BleIdentity::new([0x5a; 16]);
        let mut record = encode_persisted_ble_identity(identity);
        assert_eq!(decode_persisted_ble_identity(&record), Ok(Some(identity)));
        record[31] ^= 1;
        assert_eq!(
            decode_persisted_ble_identity(&record),
            Err(PersistedBleIdentityError::Integrity)
        );
        assert_eq!(
            decode_persisted_ble_identity(&[u8::MAX; PERSISTED_BLE_IDENTITY_LEN]),
            Ok(None)
        );
    }

    #[test]
    fn group_tag_is_stable_for_the_default_discovery_group() {
        assert_eq!(group_tag(GROUP_ID), default_group_tag());
        assert_ne!(group_tag(b"mt-leg-a"), group_tag(b"mt-leg-b"));
    }
}
