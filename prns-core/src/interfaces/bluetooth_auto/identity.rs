use crate::crypto::sha256_chunks;

pub const GROUP_ID: &[u8] = b"bluetooth-auto";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleAddress([u8; 6]);

impl BleAddress {
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    pub const fn octets(&self) -> &[u8; 6] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleIdentity([u8; 16]);

impl BleIdentity {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn from_radio_address(address: &[u8; 6]) -> Self {
        let digest = sha256_chunks(&[b"prns ble identity", address]);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}
