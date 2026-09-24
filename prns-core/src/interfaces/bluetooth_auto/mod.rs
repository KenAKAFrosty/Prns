mod advertisement;
mod backend;
mod duplex;
mod framing;
mod handshake;
mod identity;
mod policy;
mod receive;

pub use crate::interfaces::{
    DiscoveryGroupHash, DiscoveryGroupHashSet, DiscoveryGroupId, DiscoveryGroupIdError,
    DiscoveryGroupSet, DiscoveryGroupSetError, DEFAULT_DISCOVERY_GROUP_NAME, MAX_DISCOVERY_GROUPS,
    MAX_DISCOVERY_GROUP_ID_LEN,
};

pub use advertisement::{
    columba_connection_role, columba_role_capabilities,
    columba_role_capabilities_from_manufacturer, contains_service, encode_advertisement,
    BleRoleCapabilities, BleUuid, ColumbaConnectionRole, BLE_SERVICE_UUID, BLE_SERVICE_UUID_BYTES,
    COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID, COLUMBA_TX_UUID, MAX_ADVERTISEMENT_LEN,
    NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
pub use backend::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};
pub use duplex::{receive_frames_during, send_frame_duplex, BleDuplexOutcome, BleFrameForwarder};
pub use framing::{
    encode_stream_frame, fragments_of, Fragment, FragmentKind, Reassembler, StreamDeframer,
    BLE_HW_MTU, BLE_WIRE_FRAME_LEN, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
pub use handshake::{
    is_keeper, l2cap_arrangement, l2cap_plan, needs_redial, we_should_be_central, AndroidHost,
    AppleHost, BlueZHost, CloseReason, Control, ControlParseError, Endpoint, Esp32Host,
    EstablishedPeer, EstablishedTransport, Handshake, HandshakeOutcome, HandshakeReaction,
    HandshakeRole, L2capArrangement, L2capPlan, LinkCapabilities, LocalPeer, Nrf52Host,
    PeerDiscoveryGroups, PeerProtocol, Psm, WinRtHost, CONTROL_MAX_LEN,
};
pub use identity::{
    decode_persisted_ble_identity, encode_persisted_ble_identity, BleAddress, BleIdentity,
    PersistedBleIdentityError, BLE_IDENTITY_LEN, GROUP_ID, PERSISTED_BLE_IDENTITY_LEN,
};
pub use receive::{copy_received_frame, receive_frame, BleFrameReceiveError, BleReceiveError};
/// Canonical name for a Bluetooth LE device address.
pub type BluetoothLeAddress = BleAddress;
/// Canonical name for a Bluetooth LE auto-interface identity.
pub type BluetoothLeIdentity = BleIdentity;
pub use policy::{
    defaults_for_bitrate, descriptor, role_for, ConnectionPolicy, HandshakeFailureKind,
    PolicyAction, PolicyInput, BLE_BITRATE_GUESS_BPS, DIAL_FAILED_RETRY_TTL_MS, DIAL_PAUSE_MS,
    DIAL_RETRY_TTL_MS, GROUP_MISMATCH_RETRY_TTL_MS, HANDSHAKE_SLACK, HANDSHAKE_TIMEOUT_MS,
    KEEPER_DUEL_WINDOW_MS, SUPPRESS_TTL_MS,
};

#[cfg(test)]
mod tests;
