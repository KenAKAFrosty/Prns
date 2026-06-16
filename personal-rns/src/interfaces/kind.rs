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
