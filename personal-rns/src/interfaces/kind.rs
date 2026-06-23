//! The medium kind every interface declares — the namespace root of its
//! [`InterfaceId`](super::InterfaceId).
//!
//! An id is `kind ++ hash(channel_tag)`: this byte names *what kind of wire* the interface
//! is, the per-instance channel tag names *which* one. The kind namespaces the hash, so
//! two interfaces of different kinds can never collide even if their channel-tag hashes
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
    LocalServer = 9,
    LocalClient = 10,
    TcpServerPeer = 11,
    BluetoothAuto = 12,
    BluetoothPeer = 13,
    LoRa = 14,
    Kiss = 15,
    Ax25Kiss = 16,
    Pipe = 17,
    Rnode = 18,
    BackboneServer = 19,
    BackboneServerPeer = 20,
    BackboneClient = 21,
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
            9 => Some(Self::LocalServer),
            10 => Some(Self::LocalClient),
            11 => Some(Self::TcpServerPeer),
            12 => Some(Self::BluetoothAuto),
            13 => Some(Self::BluetoothPeer),
            14 => Some(Self::LoRa),
            15 => Some(Self::Kiss),
            16 => Some(Self::Ax25Kiss),
            17 => Some(Self::Pipe),
            18 => Some(Self::Rnode),
            19 => Some(Self::BackboneServer),
            20 => Some(Self::BackboneServerPeer),
            21 => Some(Self::BackboneClient),
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
            Self::LocalServer => Some(Self::LocalClient),
            Self::TcpServer => Some(Self::TcpServerPeer),
            Self::BackboneServer => Some(Self::BackboneServerPeer),
            Self::BluetoothAuto => Some(Self::BluetoothPeer),
            _ => None,
        }
    }

    /// The supervisor kind a member of this kind belongs to, if any — the inverse of
    /// [`member_kind`](Self::member_kind). A `WifiPeer` reports `AutoWifi`; a 1:1 interface (or a
    /// supervisor kind itself) reports `None`. The fan-out uses this to tell a fleet member from a
    /// dedicated interface: members of one supervisor collapse into a single broadcast the supervisor
    /// fans across its live peers, so a shared lane never has to carry a frame per member.
    #[must_use]
    pub const fn supervisor_kind(self) -> Option<Self> {
        match self {
            Self::WifiPeer => Some(Self::AutoWifi),
            Self::LocalClient => Some(Self::LocalServer),
            Self::TcpServerPeer => Some(Self::TcpServer),
            Self::BackboneServerPeer => Some(Self::BackboneServer),
            Self::BluetoothPeer => Some(Self::BluetoothAuto),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InterfaceKind;

    #[test]
    fn the_local_kinds_round_trip_their_discriminants() {
        for kind in [InterfaceKind::LocalServer, InterfaceKind::LocalClient] {
            assert_eq!(InterfaceKind::from_u8(kind as u8), Some(kind));
        }
    }

    #[test]
    fn a_local_server_supervises_local_clients() {
        assert_eq!(
            InterfaceKind::LocalServer.member_kind(),
            Some(InterfaceKind::LocalClient)
        );
        assert_eq!(InterfaceKind::LocalClient.member_kind(), None);
    }

    #[test]
    fn a_backbone_server_supervises_backbone_peers() {
        assert_eq!(
            InterfaceKind::from_u8(19),
            Some(InterfaceKind::BackboneServer)
        );
        assert_eq!(
            InterfaceKind::from_u8(20),
            Some(InterfaceKind::BackboneServerPeer)
        );
        assert_eq!(
            InterfaceKind::from_u8(21),
            Some(InterfaceKind::BackboneClient)
        );
        assert_eq!(
            InterfaceKind::BackboneServer.member_kind(),
            Some(InterfaceKind::BackboneServerPeer)
        );
        assert_eq!(
            InterfaceKind::BackboneServerPeer.supervisor_kind(),
            Some(InterfaceKind::BackboneServer)
        );
        // The outbound connector is a 1:1 interface, like TcpClient — it owns no fleet.
        assert_eq!(InterfaceKind::BackboneClient.member_kind(), None);
        assert_eq!(InterfaceKind::BackboneClient.supervisor_kind(), None);
    }

    #[test]
    fn bluetooth_auto_supervises_bluetooth_peers() {
        assert_eq!(
            InterfaceKind::from_u8(12),
            Some(InterfaceKind::BluetoothAuto)
        );
        assert_eq!(
            InterfaceKind::from_u8(13),
            Some(InterfaceKind::BluetoothPeer)
        );
        assert_eq!(
            InterfaceKind::BluetoothAuto.member_kind(),
            Some(InterfaceKind::BluetoothPeer)
        );
        assert_eq!(
            InterfaceKind::BluetoothPeer.supervisor_kind(),
            Some(InterfaceKind::BluetoothAuto)
        );
    }
}
