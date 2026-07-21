mod limits;
mod policy;
mod protocol;

pub use limits::{
    FRAMED_LEN, HANDSHAKE_TIMEOUT_MILLIS, MULTIPATH_DEDUPLICATION_CAPACITY,
    MULTIPATH_DEDUPLICATION_MILLIS, PEERING_TIMEOUT_MILLIS, READ_BUF_LEN, WDCL_MAX_CHUNK,
};
pub use policy::{
    configured_policy, descriptor, DEFAULTS, WEAVE_BITRATE_ESTIMATE, WEAVE_HW_MTU,
    WEAVE_MAX_WIRE_PACKET, WEAVE_SERIAL_BAUD,
};
pub use protocol::{
    decode_device_frame, encode_discovery, encode_discovery_response, encode_endpoint_packet,
    encode_handshake, DecodeError, DeviceEvent, EncodeError, EndpointId, MultipathDeduplicator,
    SwitchId, WeaveHostIdentity, BROADCAST_SWITCH, COMMAND_ENDPOINT_PACKET, EVENT_WDCL_CONNECTION,
    EVENT_WDCL_HOST_ENDPOINT, EVENT_WEAVE_ENDPOINT_ALIVE, EVENT_WEAVE_ENDPOINT_TIMEOUT,
    EVENT_WEAVE_ENDPOINT_VIA, TYPE_COMMAND, TYPE_CONNECT, TYPE_DISCOVER, TYPE_DISPLAY,
    TYPE_ENCAPSULATED_PROTOCOL, TYPE_ENDPOINT_PACKET, TYPE_LOG,
};
