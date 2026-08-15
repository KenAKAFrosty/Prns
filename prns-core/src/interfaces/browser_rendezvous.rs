use core::fmt;

use crate::crypto::sha256_chunks;

pub use super::local_network::{
    is_local_address, is_same_subnet, local_address_scope, LocalAddressScope,
};

pub const PORT: u16 = 42_721;
pub const PATH: &str = "/prns";
pub const CATALOG_PATH: &str = "/.well-known/prns-transport";
pub const SUBPROTOCOL: &str = "prns.transport.v1";
pub const DNS_SD_SERVICE_TYPE: &str = "_prns-ws._tcp.local.";
pub const PROTOCOL_VERSION: u16 = 1;
pub const ID_LEN: usize = 16;
pub const ID_HEX_LEN: usize = ID_LEN * 2;
pub const CLIENT_HELLO_LEN: usize = 10;
pub const SERVER_HELLO_LEN: usize = CLIENT_HELLO_LEN + ID_LEN;
pub const MAX_GATEWAYS: usize = 3;

const HELLO_MAGIC: [u8; 8] = *b"PRNSWS\0\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserRendezvousId([u8; ID_LEN]);

impl BrowserRendezvousId {
    pub const fn new(bytes: [u8; ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ID_LEN] {
        &self.0
    }

    pub fn from_lower_hex(value: &str) -> Result<Self, BrowserRendezvousIdParseError> {
        if value.len() != ID_HEX_LEN {
            return Err(BrowserRendezvousIdParseError::Length {
                actual: value.len(),
            });
        }
        let mut bytes = [0u8; ID_LEN];
        let encoded = value.as_bytes();
        let mut index = 0;
        while index < ID_LEN {
            let high = lower_hex_nibble(encoded[index * 2])
                .ok_or(BrowserRendezvousIdParseError::Character { index: index * 2 })?;
            let low = lower_hex_nibble(encoded[index * 2 + 1]).ok_or(
                BrowserRendezvousIdParseError::Character {
                    index: index * 2 + 1,
                },
            )?;
            bytes[index] = high << 4 | low;
            index += 1;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for BrowserRendezvousId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserRendezvousIdParseError {
    Length { actual: usize },
    Character { index: usize },
}

impl fmt::Display for BrowserRendezvousIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length { actual } => {
                write!(
                    formatter,
                    "browser rendezvous ID has {actual} hex digits, not {ID_HEX_LEN}"
                )
            }
            Self::Character { index } => {
                write!(
                    formatter,
                    "browser rendezvous ID has a non-lowercase-hex digit at {index}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BrowserRendezvousIdParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserSelectionSeed([u8; ID_LEN]);

impl BrowserSelectionSeed {
    pub const fn new(bytes: [u8; ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ID_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientHello;

impl ClientHello {
    pub const fn encode() -> [u8; CLIENT_HELLO_LEN] {
        let version = PROTOCOL_VERSION.to_be_bytes();
        [
            HELLO_MAGIC[0],
            HELLO_MAGIC[1],
            HELLO_MAGIC[2],
            HELLO_MAGIC[3],
            HELLO_MAGIC[4],
            HELLO_MAGIC[5],
            HELLO_MAGIC[6],
            HELLO_MAGIC[7],
            version[0],
            version[1],
        ]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HelloDecodeError> {
        let version = decode_hello_prefix(bytes, CLIENT_HELLO_LEN)?;
        if version != PROTOCOL_VERSION {
            return Err(HelloDecodeError::UnsupportedVersion(version));
        }
        Ok(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerHello {
    id: BrowserRendezvousId,
}

impl ServerHello {
    pub const fn new(id: BrowserRendezvousId) -> Self {
        Self { id }
    }

    pub const fn id(&self) -> BrowserRendezvousId {
        self.id
    }

    pub fn encode(&self) -> [u8; SERVER_HELLO_LEN] {
        let mut bytes = [0u8; SERVER_HELLO_LEN];
        bytes[..CLIENT_HELLO_LEN].copy_from_slice(&ClientHello::encode());
        bytes[CLIENT_HELLO_LEN..].copy_from_slice(self.id.as_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HelloDecodeError> {
        let version = decode_hello_prefix(bytes, SERVER_HELLO_LEN)?;
        if version != PROTOCOL_VERSION {
            return Err(HelloDecodeError::UnsupportedVersion(version));
        }
        let mut id = [0u8; ID_LEN];
        id.copy_from_slice(&bytes[CLIENT_HELLO_LEN..]);
        Ok(Self::new(BrowserRendezvousId::new(id)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloDecodeError {
    Length { expected: usize, actual: usize },
    Magic,
    UnsupportedVersion(u16),
}

impl fmt::Display for HelloDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length { expected, actual } => {
                write!(
                    formatter,
                    "rendezvous hello has {actual} bytes, not {expected}"
                )
            }
            Self::Magic => formatter.write_str("rendezvous hello has the wrong protocol magic"),
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "rendezvous hello version {version} is unsupported"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for HelloDecodeError {}

#[must_use]
pub fn gateway_weight(seed: BrowserSelectionSeed, id: BrowserRendezvousId) -> u128 {
    let digest = sha256_chunks(&[
        b"prns browser gateway selection v1",
        seed.as_bytes(),
        id.as_bytes(),
    ]);
    let mut weight = [0u8; 16];
    weight.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(weight)
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn decode_hello_prefix(bytes: &[u8], expected: usize) -> Result<u16, HelloDecodeError> {
    if bytes.len() != expected {
        return Err(HelloDecodeError::Length {
            expected,
            actual: bytes.len(),
        });
    }
    if bytes[..HELLO_MAGIC.len()] != HELLO_MAGIC {
        return Err(HelloDecodeError::Magic);
    }
    Ok(u16::from_be_bytes([
        bytes[HELLO_MAGIC.len()],
        bytes[HELLO_MAGIC.len() + 1],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendezvous_ids_round_trip_only_canonical_lower_hex() {
        let id = BrowserRendezvousId::new([
            0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0x77,
        ]);
        let rendered = id.to_string();
        assert_eq!(rendered, "00123456789abcdef011223344556677");
        assert_eq!(BrowserRendezvousId::from_lower_hex(&rendered), Ok(id));
        assert!(matches!(
            BrowserRendezvousId::from_lower_hex("00123456789ABCDEF011223344556677"),
            Err(BrowserRendezvousIdParseError::Character { .. })
        ));
    }

    #[test]
    fn client_and_server_hellos_are_exact_and_versioned() {
        assert_eq!(ClientHello::decode(&ClientHello::encode()), Ok(ClientHello));
        let id = BrowserRendezvousId::new([0x5a; ID_LEN]);
        let hello = ServerHello::new(id);
        assert_eq!(ServerHello::decode(&hello.encode()), Ok(hello));

        let mut wrong_version = ClientHello::encode();
        wrong_version[CLIENT_HELLO_LEN - 1] = 2;
        assert_eq!(
            ClientHello::decode(&wrong_version),
            Err(HelloDecodeError::UnsupportedVersion(2))
        );
        assert_eq!(
            ClientHello::decode(&ClientHello::encode()[..CLIENT_HELLO_LEN - 1]),
            Err(HelloDecodeError::Length {
                expected: CLIENT_HELLO_LEN,
                actual: CLIENT_HELLO_LEN - 1,
            })
        );
    }

    #[test]
    fn gateway_ranking_is_stable_and_identity_sensitive() {
        let seed = BrowserSelectionSeed::new([0x11; ID_LEN]);
        let first = BrowserRendezvousId::new([0x22; ID_LEN]);
        let second = BrowserRendezvousId::new([0x23; ID_LEN]);
        assert_eq!(gateway_weight(seed, first), gateway_weight(seed, first));
        assert_ne!(gateway_weight(seed, first), gateway_weight(seed, second));
    }
}
