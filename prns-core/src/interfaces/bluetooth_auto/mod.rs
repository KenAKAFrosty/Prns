mod advertisement;
mod backend;
mod framing;
mod handshake;
mod identity;
mod policy;

pub use advertisement::{
    advertisement_group_tag, columba_connection_role, columba_role_capabilities,
    columba_role_capabilities_from_manufacturer, contains_service, dial_key_from_identity,
    dial_key_from_manufacturer, discovery_groups_match, encode_advertisement,
    group_tag_from_manufacturer, manufacturer_discovery_group_tag,
    manufacturer_discovery_groups_match, manufacturer_role_payload,
    manufacturer_role_payload_with_dial_key, BleRoleCapabilities, BleUuid, ColumbaConnectionRole,
    BLE_SERVICE_UUID, BLE_SERVICE_UUID_BYTES, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID,
    COLUMBA_TX_UUID, DIAL_KEY_LEN, MAX_ADVERTISEMENT_LEN, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
pub use backend::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};
pub use framing::{
    encode_stream_frame, fragments_of, Fragment, FragmentKind, Reassembler, StreamDeframer,
    BLE_HW_MTU, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
pub use handshake::{
    is_keeper, l2cap_arrangement, l2cap_plan, needs_redial, resolved_discovery_group,
    we_should_be_central, AndroidHost, AppleHost, BlueZHost, CloseReason, Control, Endpoint,
    Esp32Host, EstablishedPeer, EstablishedTransport, Handshake, HandshakeOutcome,
    HandshakeReaction, HandshakeRole, L2capArrangement, L2capPlan, LinkCapabilities, LocalPeer,
    Nrf52Host, PeerProtocol, Psm, WinRtHost, CONTROL_LEGACY_GREETING_LEN, CONTROL_MAX_LEN,
};
pub use identity::{
    decode_persisted_ble_identity, default_group_tag, encode_persisted_ble_identity, group_tag,
    BleAddress, BleIdentity, PersistedBleIdentityError, BLE_IDENTITY_LEN, CHANNEL_TAG,
    DEFAULT_GROUP_TAG, GROUP_ID, GROUP_NAME, GROUP_TAG_LEN, PERSISTED_BLE_IDENTITY_LEN,
};
/// Canonical name for a Bluetooth LE device address.
pub type BluetoothLeAddress = BleAddress;
/// Canonical name for a Bluetooth LE auto-interface identity.
pub type BluetoothLeIdentity = BleIdentity;
pub use policy::{
    defaults_for_bitrate, descriptor, role_for, ConnectionPolicy, PolicyAction, PolicyInput,
    BLE_BITRATE_GUESS_BPS, DIAL_FAILED_RETRY_TTL_MS, DIAL_PAUSE_MS, DIAL_RETRY_TTL_MS,
    HANDSHAKE_SLACK, KEEPER_DUEL_WINDOW_MS, SUPPRESS_TTL_MS,
};

#[cfg(test)]
mod tests;
