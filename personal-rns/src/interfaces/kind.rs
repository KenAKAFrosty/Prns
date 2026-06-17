//! The medium kind every interface declares — the namespace root of its
//! [`InterfaceId`](super::InterfaceId).
//!
//! An id is `kind ++ hash(reachability_tag)`: this byte names *what kind of wire* the interface
//! is, the per-instance reachability tag names *which* one. The kind namespaces the hash, so
//! two interfaces of different kinds can never collide even if their reachability-tag hashes
//! did, and the byte makes an id self-describing (a face can read the medium straight off it).
//! Supervisors and the fleet members they stand up are distinct kinds.
//!
//! The discriminant is written into every id (and, once routes persist, onto disk), so it is a
//! stable wire-like contract: never renumber a variant, only append. Renumbering would silently
//! repoint every persisted route of that kind.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InterfaceKind {
    Loopback = 0,
    TcpClient = 1,
    TcpServer = 2,
    Udp = 3,
    Serial = 4,
    UsbAutoHost = 5,
    UsbAutoDevice = 6,
    AutoWifi = 7,
    WifiPeer = 8,
}

impl InterfaceKind {
    /// Recover the kind from an id's first byte. `None` for an unknown discriminant — the byte is
    /// data the kind never renumbers, so an unrecognized value is a foreign or corrupt id, not a
    /// kind to guess at.
    #[must_use]
    pub const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Loopback),
            1 => Some(Self::TcpClient),
            2 => Some(Self::TcpServer),
            3 => Some(Self::Udp),
            4 => Some(Self::Serial),
            5 => Some(Self::UsbAutoHost),
            6 => Some(Self::UsbAutoDevice),
            7 => Some(Self::AutoWifi),
            8 => Some(Self::WifiPeer),
            _ => None,
        }
    }

    /// The kind of child a supervisor of this kind stands up, if it is a fleet supervisor. A fleet
    /// lane registered under a supervisor's id serves every child of this kind, so the reactor
    /// routes a child's frames to that one lane by the kind byte alone — no per-child entry. `None`
    /// for a kind that owns no fleet (a 1:1 interface, or a member kind itself).
    #[must_use]
    pub const fn member_kind(self) -> Option<Self> {
        match self {
            Self::AutoWifi => Some(Self::WifiPeer),
            _ => None,
        }
    }
}
