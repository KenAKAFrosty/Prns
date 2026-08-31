use crate::entropy::{EntropySource, RuntimeEntropy};

pub const GROUP_ID: &[u8] = b"bluetooth-auto";
pub const BLE_IDENTITY_LEN: usize = 16;
pub const PERSISTED_BLE_IDENTITY_LEN: usize = 40;

const PERSISTED_BLE_IDENTITY_MAGIC: [u8; 8] = *b"PRNSBLE1";

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

    #[must_use]
    pub fn generate<S: EntropySource>(entropy: &mut RuntimeEntropy<S>) -> Self {
        let mut bytes = [0_u8; BLE_IDENTITY_LEN];
        entropy.fill_random(&mut bytes);
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
    use core::convert::Infallible;

    use super::*;

    struct TestEntropySource(u8);

    impl EntropySource for TestEntropySource {
        type Error = Infallible;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            output.fill(self.0);
            Ok(())
        }
    }

    fn runtime_entropy(seed: u8) -> RuntimeEntropy<TestEntropySource> {
        RuntimeEntropy::try_new(TestEntropySource(seed)).unwrap()
    }

    #[test]
    fn generated_identity_comes_from_the_runtime_stream() {
        let mut first_entropy = runtime_entropy(0x41);
        let mut reference_entropy = runtime_entropy(0x41);

        let first = BleIdentity::generate(&mut first_entropy);
        let reference = BleIdentity::generate(&mut reference_entropy);
        let second = BleIdentity::generate(&mut first_entropy);

        assert_eq!(first, reference);
        assert_ne!(first, second);
    }

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
}
