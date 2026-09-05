use personal_rns::interfaces::{
    ConfiguredInterfacePolicy, InterfaceCommonPolicy, InterfaceDescriptor,
    InterfaceForwardingPolicy, InterfaceMode,
};

/// Whether Boundary-learned announces may cross onto Internal faces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnnouncesToInternal {
    Denied,
    Allowed,
}

impl AnnouncesToInternal {
    #[must_use]
    pub const fn from_allowed(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Denied
        }
    }

    #[must_use]
    pub const fn allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Denied => "No",
            Self::Allowed => "Yes",
        }
    }
}

/// Per-face Transport mode preference for a Hopspot interface card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceModeSelection {
    pub mode: InterfaceMode,
    pub announces_to_internal: AnnouncesToInternal,
}

impl InterfaceModeSelection {
    pub const DEFAULT: Self = Self {
        mode: InterfaceMode::Full,
        announces_to_internal: AnnouncesToInternal::Denied,
    };

    #[must_use]
    pub const fn full() -> Self {
        Self::DEFAULT
    }

    #[must_use]
    pub fn for_mode(mode: InterfaceMode) -> Self {
        Self {
            mode,
            announces_to_internal: AnnouncesToInternal::Denied,
        }
    }

    #[must_use]
    pub fn configured_policy(self) -> ConfiguredInterfacePolicy {
        let mut common = InterfaceCommonPolicy::RNS_DEFAULT;
        common.forwarding = InterfaceForwardingPolicy {
            announces_to_internal: self.announces_to_internal.allowed(),
            ..common.forwarding
        };
        ConfiguredInterfacePolicy {
            mode: Some(self.mode),
            common: Some(common),
            ..ConfiguredInterfacePolicy::default()
        }
    }
}

/// Durable slots for independent Hopspot interface cards (not fleet members).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum InterfaceModeSlot {
    Wifi = 0,
    Usb = 1,
    Ble = 2,
    LoRa = 3,
    EspNow = 4,
    SharedInstance = 5,
    Tcp = 6,
}

pub const INTERFACE_MODE_SLOT_COUNT: usize = 7;

impl InterfaceModeSlot {
    pub const ALL: [Self; INTERFACE_MODE_SLOT_COUNT] = [
        Self::Wifi,
        Self::Usb,
        Self::Ble,
        Self::LoRa,
        Self::EspNow,
        Self::SharedInstance,
        Self::Tcp,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Wifi),
            1 => Some(Self::Usb),
            2 => Some(Self::Ble),
            3 => Some(Self::LoRa),
            4 => Some(Self::EspNow),
            5 => Some(Self::SharedInstance),
            6 => Some(Self::Tcp),
            _ => None,
        }
    }
}

#[must_use]
pub const fn interface_mode_label(mode: InterfaceMode) -> &'static str {
    match mode {
        InterfaceMode::Full => "Full",
        InterfaceMode::PointToPoint => "Point-to-Point",
        InterfaceMode::AccessPoint => "Access Point",
        InterfaceMode::Roaming => "Roaming",
        InterfaceMode::Boundary => "Boundary",
        InterfaceMode::Gateway => "Gateway",
        InterfaceMode::Internal => "Internal",
    }
}

#[must_use]
pub const fn interface_mode_menu_label(mode: InterfaceMode) -> &'static str {
    match mode {
        InterfaceMode::Full => "Full",
        InterfaceMode::PointToPoint => "PtP",
        InterfaceMode::AccessPoint => "AP",
        InterfaceMode::Roaming => "Roam",
        InterfaceMode::Boundary => "Bound",
        InterfaceMode::Gateway => "Gate",
        InterfaceMode::Internal => "Int",
    }
}

pub const INTERFACE_MODE_CHOICES: [InterfaceMode; 7] = [
    InterfaceMode::Full,
    InterfaceMode::PointToPoint,
    InterfaceMode::AccessPoint,
    InterfaceMode::Roaming,
    InterfaceMode::Boundary,
    InterfaceMode::Gateway,
    InterfaceMode::Internal,
];

/// Working set of per-slot mode preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceModeTable {
    slots: [InterfaceModeSelection; INTERFACE_MODE_SLOT_COUNT],
}

impl InterfaceModeTable {
    pub const DEFAULT: Self = Self {
        slots: [InterfaceModeSelection::DEFAULT; INTERFACE_MODE_SLOT_COUNT],
    };

    #[must_use]
    pub const fn new() -> Self {
        Self::DEFAULT
    }

    #[must_use]
    pub fn get(self, slot: InterfaceModeSlot) -> InterfaceModeSelection {
        self.slots[slot.index()]
    }

    pub fn set(&mut self, slot: InterfaceModeSlot, selection: InterfaceModeSelection) {
        self.slots[slot.index()] = selection;
    }
}

impl Default for InterfaceModeTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply a Hopspot mode preference onto an already-built descriptor.
pub fn apply_selection_to_descriptor(
    descriptor: &mut InterfaceDescriptor,
    selection: InterfaceModeSelection,
) {
    descriptor.mode = selection.mode;
    descriptor.common.forwarding.announces_to_internal = selection.announces_to_internal.allowed();
}

/// Map an interface kind onto a durable Hopspot mode slot, when one exists.
#[must_use]
pub fn interface_mode_slot_for_kind(
    kind: Option<personal_rns::interfaces::InterfaceKind>,
) -> Option<InterfaceModeSlot> {
    match kind {
        Some(
            personal_rns::interfaces::InterfaceKind::AutoWifi
            | personal_rns::interfaces::InterfaceKind::WifiPeer
            | personal_rns::interfaces::InterfaceKind::WifiDirect
            | personal_rns::interfaces::InterfaceKind::WifiDirectPeer,
        ) => Some(InterfaceModeSlot::Wifi),
        Some(
            personal_rns::interfaces::InterfaceKind::UsbAutoHost
            | personal_rns::interfaces::InterfaceKind::UsbAutoDevice,
        ) => Some(InterfaceModeSlot::Usb),
        Some(
            personal_rns::interfaces::InterfaceKind::BluetoothAuto
            | personal_rns::interfaces::InterfaceKind::BluetoothPeer,
        ) => Some(InterfaceModeSlot::Ble),
        Some(personal_rns::interfaces::InterfaceKind::LoRa) => Some(InterfaceModeSlot::LoRa),
        Some(personal_rns::interfaces::InterfaceKind::EspNow) => Some(InterfaceModeSlot::EspNow),
        Some(
            personal_rns::interfaces::InterfaceKind::LocalServer
            | personal_rns::interfaces::InterfaceKind::LocalClient,
        ) => Some(InterfaceModeSlot::SharedInstance),
        Some(
            personal_rns::interfaces::InterfaceKind::TcpClient
            | personal_rns::interfaces::InterfaceKind::TcpServer
            | personal_rns::interfaces::InterfaceKind::TcpServerPeer,
        ) => Some(InterfaceModeSlot::Tcp),
        _ => None,
    }
}

/// Resolve the display/transport mode for a snapshot from the preference table.
#[must_use]
pub fn mode_from_table(
    table: InterfaceModeTable,
    kind: Option<personal_rns::interfaces::InterfaceKind>,
) -> InterfaceMode {
    interface_mode_slot_for_kind(kind)
        .map(|slot| table.get(slot).mode)
        .unwrap_or(InterfaceMode::Full)
}

#[cfg(feature = "host")]
const HOST_INTERFACE_MODE_MAGIC: [u8; 4] = *b"HSIM";
#[cfg(feature = "host")]
const HOST_INTERFACE_MODE_VERSION: u16 = 1;
#[cfg(feature = "host")]
const HOST_INTERFACE_MODE_BYTES: usize = 4 + 2 + INTERFACE_MODE_SLOT_COUNT * 2;

/// Host-side filename for the durable interface-mode preference table.
#[cfg(feature = "host")]
pub const INTERFACE_MODE_STORAGE: &str = "interface_modes";

/// Load a host-side preference table, or DEFAULT when missing/malformed.
#[cfg(feature = "host")]
#[must_use]
pub fn load_host_interface_modes(path: &std::path::Path) -> InterfaceModeTable {
    let Ok(bytes) = std::fs::read(path) else {
        return InterfaceModeTable::DEFAULT;
    };
    decode_host_interface_modes(&bytes).unwrap_or(InterfaceModeTable::DEFAULT)
}

/// Persist a host-side preference table beside identity files.
#[cfg(feature = "host")]
pub fn save_host_interface_modes(
    path: &std::path::Path,
    table: InterfaceModeTable,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, encode_host_interface_modes(table))
}

#[cfg(feature = "host")]
fn encode_host_interface_modes(table: InterfaceModeTable) -> [u8; HOST_INTERFACE_MODE_BYTES] {
    let mut bytes = [0u8; HOST_INTERFACE_MODE_BYTES];
    bytes[..4].copy_from_slice(&HOST_INTERFACE_MODE_MAGIC);
    bytes[4..6].copy_from_slice(&HOST_INTERFACE_MODE_VERSION.to_le_bytes());
    for (index, slot) in InterfaceModeSlot::ALL.into_iter().enumerate() {
        let selection = table.get(slot);
        let offset = 6 + index * 2;
        bytes[offset] = encode_host_mode(selection.mode);
        bytes[offset + 1] = u8::from(selection.announces_to_internal.allowed());
    }
    bytes
}

#[cfg(feature = "host")]
fn decode_host_interface_modes(bytes: &[u8]) -> Option<InterfaceModeTable> {
    if bytes.len() < HOST_INTERFACE_MODE_BYTES {
        return None;
    }
    if bytes[..4] != HOST_INTERFACE_MODE_MAGIC {
        return None;
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != HOST_INTERFACE_MODE_VERSION {
        return None;
    }
    let mut table = InterfaceModeTable::DEFAULT;
    for index in 0..INTERFACE_MODE_SLOT_COUNT {
        let offset = 6 + index * 2;
        let mode = decode_host_mode(bytes[offset])?;
        let announces = AnnouncesToInternal::from_allowed(bytes[offset + 1] != 0);
        let slot = InterfaceModeSlot::from_index(index)?;
        table.set(
            slot,
            InterfaceModeSelection {
                mode,
                announces_to_internal: announces,
            },
        );
    }
    Some(table)
}

#[cfg(feature = "host")]
fn encode_host_mode(mode: InterfaceMode) -> u8 {
    match mode {
        InterfaceMode::Full => 0,
        InterfaceMode::PointToPoint => 1,
        InterfaceMode::AccessPoint => 2,
        InterfaceMode::Roaming => 3,
        InterfaceMode::Boundary => 4,
        InterfaceMode::Gateway => 5,
        InterfaceMode::Internal => 6,
    }
}

#[cfg(feature = "host")]
fn decode_host_mode(value: u8) -> Option<InterfaceMode> {
    Some(match value {
        0 => InterfaceMode::Full,
        1 => InterfaceMode::PointToPoint,
        2 => InterfaceMode::AccessPoint,
        3 => InterfaceMode::Roaming,
        4 => InterfaceMode::Boundary,
        5 => InterfaceMode::Gateway,
        6 => InterfaceMode::Internal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_policy_carries_announces_to_internal() {
        let selection = InterfaceModeSelection {
            mode: InterfaceMode::Boundary,
            announces_to_internal: AnnouncesToInternal::Allowed,
        };
        let policy = selection.configured_policy();
        assert_eq!(policy.mode, Some(InterfaceMode::Boundary));
        assert!(
            policy
                .common
                .expect("common")
                .forwarding
                .announces_to_internal
        );
    }

    #[test]
    fn mode_from_table_maps_usb_and_defaults_unknown() {
        let mut table = InterfaceModeTable::DEFAULT;
        table.set(
            InterfaceModeSlot::Usb,
            InterfaceModeSelection::for_mode(InterfaceMode::Gateway),
        );
        assert_eq!(
            mode_from_table(
                table,
                Some(personal_rns::interfaces::InterfaceKind::UsbAutoDevice)
            ),
            InterfaceMode::Gateway
        );
        assert_eq!(
            mode_from_table(
                table,
                Some(personal_rns::interfaces::InterfaceKind::Loopback)
            ),
            InterfaceMode::Full
        );
    }

    #[cfg(feature = "host")]
    #[test]
    fn host_interface_modes_round_trip() {
        let mut table = InterfaceModeTable::DEFAULT;
        table.set(
            InterfaceModeSlot::LoRa,
            InterfaceModeSelection {
                mode: InterfaceMode::Boundary,
                announces_to_internal: AnnouncesToInternal::Allowed,
            },
        );
        let encoded = encode_host_interface_modes(table);
        let decoded = decode_host_interface_modes(&encoded).expect("decode");
        assert_eq!(
            decoded.get(InterfaceModeSlot::LoRa),
            table.get(InterfaceModeSlot::LoRa)
        );
    }
}
