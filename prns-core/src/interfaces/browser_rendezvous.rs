use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::crypto::sha256_chunks;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAddressScope {
    Loopback,
    Private,
    LinkLocal,
}

#[must_use]
pub fn local_address_scope(address: IpAddr) -> Option<LocalAddressScope> {
    match address {
        IpAddr::V4(address) => ipv4_scope(address),
        IpAddr::V6(address) => ipv6_scope(address),
    }
}

#[must_use]
pub fn is_local_address(address: IpAddr) -> bool {
    local_address_scope(address).is_some()
}

#[must_use]
pub fn is_same_subnet(local: IpAddr, netmask: IpAddr, peer: IpAddr) -> bool {
    if local.is_loopback() && peer.is_loopback() {
        return true;
    }
    if local_address_scope(local).is_none() || local_address_scope(peer).is_none() {
        return false;
    }
    match (local, netmask, peer) {
        (IpAddr::V4(local), IpAddr::V4(mask), IpAddr::V4(peer)) => {
            u32::from(local) & u32::from(mask) == u32::from(peer) & u32::from(mask)
        }
        (IpAddr::V6(local), IpAddr::V6(mask), IpAddr::V6(peer)) => {
            u128::from(local) & u128::from(mask) == u128::from(peer) & u128::from(mask)
        }
        (IpAddr::V4(_), IpAddr::V4(_), IpAddr::V6(_))
        | (IpAddr::V4(_), IpAddr::V6(_), IpAddr::V4(_))
        | (IpAddr::V4(_), IpAddr::V6(_), IpAddr::V6(_))
        | (IpAddr::V6(_), IpAddr::V4(_), IpAddr::V4(_))
        | (IpAddr::V6(_), IpAddr::V4(_), IpAddr::V6(_))
        | (IpAddr::V6(_), IpAddr::V6(_), IpAddr::V4(_)) => false,
    }
}

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

fn ipv4_scope(address: Ipv4Addr) -> Option<LocalAddressScope> {
    if address.is_loopback() {
        return Some(LocalAddressScope::Loopback);
    }
    if address.is_private() {
        return Some(LocalAddressScope::Private);
    }
    if address.is_link_local() {
        return Some(LocalAddressScope::LinkLocal);
    }
    None
}

fn ipv6_scope(address: Ipv6Addr) -> Option<LocalAddressScope> {
    if address.is_loopback() {
        return Some(LocalAddressScope::Loopback);
    }
    if address.is_unique_local() {
        return Some(LocalAddressScope::Private);
    }
    if address.is_unicast_link_local() {
        return Some(LocalAddressScope::LinkLocal);
    }
    None
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
    fn only_explicit_local_unicast_ranges_are_eligible() {
        let accepted = [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.255.254",
            "169.254.1.2",
            "::1",
            "fc00::1",
            "fdff::1",
            "fe80::1",
        ];
        let rejected = [
            "0.0.0.0",
            "8.8.8.8",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "2001:4860:4860::8888",
            "ff02::1",
        ];
        for address in accepted {
            let address = address.parse().expect("test address parses");
            assert!(is_local_address(address), "{address} must be local");
        }
        for address in rejected {
            let address = address.parse().expect("test address parses");
            assert!(!is_local_address(address), "{address} must be rejected");
        }
    }

    #[test]
    fn subnet_validation_rejects_other_private_and_public_networks() {
        assert!(is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "192.168.4.99".parse().unwrap(),
        ));
        assert!(!is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "192.168.5.99".parse().unwrap(),
        ));
        assert!(!is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "8.8.8.8".parse().unwrap(),
        ));
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
