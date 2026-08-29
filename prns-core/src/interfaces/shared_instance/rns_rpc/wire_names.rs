pub(super) mod dialect {
    pub const PICKLE: &str = "pickle";
    pub const MESSAGE_PACK: &str = "msgpack";
}

pub(super) mod digest {
    pub const MD5: &[u8] = b"md5";
    pub const SHA256: &[u8] = b"sha256";
}

pub(super) mod selector {
    pub const GET: &str = "get";
    pub const DROP: &str = "drop";
    pub const BLACKHOLE_IDENTITY: &str = "blackhole_identity";
    pub const UNBLACKHOLE_IDENTITY: &str = "unblackhole_identity";
    pub const DESTINATION_DATA: &str = "destination_data";
    pub const IDENTITY_DATA: &str = "identity_data";
}

pub(super) mod argument {
    use crate::interfaces::rns_management::wire_names::common;

    pub const MAX_HOPS: &str = "max_hops";
    pub const DESTINATION_HASH: &str = "destination_hash";
    pub const PACKET_HASH: &str = "packet_hash";
    pub const IDENTITY_HASH: &str = "identity_hash";
    pub const UNTIL: &str = common::UNTIL;
    pub const REASON: &str = common::REASON;
}

pub(super) mod get {
    pub const INTERFACE_STATS: &str = "interface_stats";
    pub const PATH_TABLE: &str = "path_table";
    pub const RATE_TABLE: &str = "rate_table";
    pub const NEXT_HOP_INTERFACE_NAME: &str = "next_hop_if_name";
    pub const NEXT_HOP: &str = "next_hop";
    pub const FIRST_HOP_TIMEOUT: &str = "first_hop_timeout";
    pub const LOWEST_INTERFACE_BITRATE: &str = "lowest_interface_bitrate";
    pub const MEDIUM_PATH_TIMEOUT: &str = "medium_path_timeout";
    pub const LINK_COUNT: &str = "link_count";
    pub const PACKET_RSSI: &str = "packet_rssi";
    pub const PACKET_SNR: &str = "packet_snr";
    pub const PACKET_QUALITY: &str = "packet_q";
    pub const BLACKHOLED_IDENTITIES: &str = "blackholed_identities";
    pub const IS_BLACKHOLED: &str = "is_blackholed";
}

pub(super) mod drop_operation {
    pub const PATH: &str = "path";
    pub const ALL_VIA: &str = "all_via";
    pub const ANNOUNCE_QUEUES: &str = "announce_queues";
}

pub(super) mod data_operation {
    pub const USED: &str = "used";
    pub const RETAIN: &str = "retain";
    pub const UNRETAIN: &str = "unretain";
}

pub(super) mod reply_value {
    pub const NO_INTERFACE: &str = "None";
}

pub(super) mod verb {
    use super::{get, selector};

    pub const GET_INTERFACE_STATS: &str = get::INTERFACE_STATS;
    pub const GET_PATH_TABLE: &str = get::PATH_TABLE;
    pub const GET_RATE_TABLE: &str = get::RATE_TABLE;
    pub const GET_LINK_COUNT: &str = get::LINK_COUNT;
    pub const GET_NEXT_HOP: &str = get::NEXT_HOP;
    pub const GET_NEXT_HOP_INTERFACE_NAME: &str = get::NEXT_HOP_INTERFACE_NAME;
    pub const GET_FIRST_HOP_TIMEOUT: &str = get::FIRST_HOP_TIMEOUT;
    pub const GET_LOWEST_INTERFACE_BITRATE: &str = get::LOWEST_INTERFACE_BITRATE;
    pub const GET_MEDIUM_PATH_TIMEOUT: &str = get::MEDIUM_PATH_TIMEOUT;
    pub const GET_PACKET_RSSI: &str = get::PACKET_RSSI;
    pub const GET_PACKET_SNR: &str = get::PACKET_SNR;
    pub const GET_PACKET_QUALITY: &str = get::PACKET_QUALITY;
    pub const GET_BLACKHOLED_IDENTITIES: &str = get::BLACKHOLED_IDENTITIES;
    pub const CHECK_IDENTITY_BLACKHOLED: &str = get::IS_BLACKHOLED;
    pub const DROP_PATH: &str = "drop_path";
    pub const DROP_ALL_VIA: &str = "drop_all_via";
    pub const DROP_ANNOUNCE_QUEUES: &str = "drop_announce_queues";
    pub const BLACKHOLE_IDENTITY: &str = selector::BLACKHOLE_IDENTITY;
    pub const UNBLACKHOLE_IDENTITY: &str = selector::UNBLACKHOLE_IDENTITY;
    pub const UPDATE_DESTINATION_DATA: &str = selector::DESTINATION_DATA;
    pub const RETAIN_IDENTITY: &str = selector::IDENTITY_DATA;
    pub const UNKNOWN: &str = "unknown";
}
