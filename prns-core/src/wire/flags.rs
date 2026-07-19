#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContextFlag {
    Unset = 0b0,
    Set = 0b1,
}

impl ContextFlag {
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Unset,
            _ => Self::Set,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PropagationType {
    Broadcast = 0b0,
    Transport = 0b1,
}

impl PropagationType {
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Broadcast,
            _ => Self::Transport,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DestinationType {
    Single = 0b00,
    Group = 0b01,
    Plain = 0b10,
    Link = 0b11,
}

impl DestinationType {
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Self::Single,
            0b01 => Self::Group,
            0b10 => Self::Plain,
            _ => Self::Link,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Data = 0b00,
    Announce = 0b01,
    LinkRequest = 0b10,
    Proof = 0b11,
}

impl PacketType {
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Self::Data,
            0b01 => Self::Announce,
            0b10 => Self::LinkRequest,
            _ => Self::Proof,
        }
    }
}

/// Every writer packs Open because that is what RNS 1.3.5 does at construction. IFAC is an interface-boundary transform, where `Transport.transmit` masks the raw packet and flips this flag per interface key, and `Transport.inbound` unmasks or drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IfacFlag {
    Open = 0b0,
    Authenticated = 0b1,
}

impl IfacFlag {
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Open,
            _ => Self::Authenticated,
        }
    }
}
