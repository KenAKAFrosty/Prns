use super::limits::TRUNCATED_HASH_BYTE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireAddress([u8; TRUNCATED_HASH_BYTE_LEN]);

impl WireAddress {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationHash([u8; TRUNCATED_HASH_BYTE_LEN]);

impl DestinationHash {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }

    pub const fn from_address(address: WireAddress) -> Self {
        Self(*address.as_bytes())
    }

    pub const fn to_address(&self) -> WireAddress {
        WireAddress(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportId([u8; TRUNCATED_HASH_BYTE_LEN]);

impl TransportId {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}
