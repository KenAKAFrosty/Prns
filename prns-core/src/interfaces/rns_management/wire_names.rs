pub mod common {
    pub const HASH: &str = "hash";
    pub const UNTIL: &str = "until";
    pub const REASON: &str = "reason";
}

pub mod path {
    pub const VIA: &str = "via";
    pub const HOPS: &str = "hops";
    pub const TIMESTAMP: &str = "timestamp";
    pub const EXPIRES: &str = "expires";
    pub const INTERFACE: &str = "interface";
}

pub mod rate {
    pub const LAST: &str = "last";
    pub const VIOLATIONS: &str = "rate_violations";
    pub const BLOCKED_UNTIL: &str = "blocked_until";
    pub const TIMESTAMPS: &str = "timestamps";
}

pub mod interface {
    pub const INTERFACES: &str = "interfaces";
    pub const NAME: &str = "name";
    pub const SHORT_NAME: &str = "short_name";
    pub const TYPE: &str = "type";
    pub const STATUS: &str = "status";
    pub const MODE: &str = "mode";
    pub const GRAVITY: &str = "gravity";
    pub const CLIENTS: &str = "clients";
    pub const RECEIVE_BYTES: &str = "rxb";
    pub const TRANSMIT_BYTES: &str = "txb";
    pub const RECEIVE_SPEED: &str = "rxs";
    pub const TRANSMIT_SPEED: &str = "txs";
    pub const IFAC_SIGNATURE: &str = "ifac_signature";
    pub const IFAC_SIZE: &str = "ifac_size";
    pub const IFAC_NETWORK_NAME: &str = "ifac_netname";
    pub const RESIDENT_SET_SIZE: &str = "rss";
    pub const HASH: &str = "hash";
    pub const PARENT_NAME: &str = "parent_interface_name";
    pub const PARENT_HASH: &str = "parent_interface_hash";
    pub const BITRATE: &str = "bitrate";
    pub const PEERS: &str = "peers";
    pub const FLEET_PEERS: &str = "fleet_peers";
    pub const RSSI: &str = "rssi";
    pub const GROUP_ID: &str = "group_id";
    pub const AUTOCONNECT_SOURCE: &str = "autoconnect_source";
    pub const ANNOUNCE_QUEUE: &str = "announce_queue";
    pub const HELD_ANNOUNCES: &str = "held_announces";
    pub const INCOMING_ANNOUNCE_FREQUENCY: &str = "incoming_announce_frequency";
    pub const OUTGOING_ANNOUNCE_FREQUENCY: &str = "outgoing_announce_frequency";
    pub const INCOMING_PATH_REQUEST_FREQUENCY: &str = "incoming_pr_frequency";
    pub const OUTGOING_PATH_REQUEST_FREQUENCY: &str = "outgoing_pr_frequency";
    pub const ANNOUNCE_RATE_TARGET: &str = "announce_rate_target";
    pub const ANNOUNCE_RATE_PENALTY: &str = "announce_rate_penalty";
    pub const ANNOUNCE_RATE_GRACE: &str = "announce_rate_grace";
    pub const BURST_ACTIVE: &str = "burst_active";
    pub const BURST_ACTIVATED: &str = "burst_activated";
    pub const PATH_REQUEST_BURST_ACTIVE: &str = "pr_burst_active";
    pub const PATH_REQUEST_BURST_ACTIVATED: &str = "pr_burst_activated";
    pub const I2P_CONNECTABLE: &str = "i2p_connectable";
    pub const I2P_B32: &str = "i2p_b32";
    pub const I2P_TUNNEL_STATE: &str = "tunnelstate";
    pub const AIRTIME_SHORT: &str = "airtime_short";
    pub const AIRTIME_LONG: &str = "airtime_long";
    pub const CHANNEL_LOAD_SHORT: &str = "channel_load_short";
    pub const CHANNEL_LOAD_LONG: &str = "channel_load_long";
    pub const NOISE_FLOOR: &str = "noise_floor";
    pub const INTERFERENCE: &str = "interference";
    pub const INTERFERENCE_LAST_AT: &str = "interference_last_ts";
    pub const INTERFERENCE_LAST_DBM: &str = "interference_last_dbm";
    pub const CPU_LOAD: &str = "cpu_load";
    pub const CPU_TEMPERATURE: &str = "cpu_temp";
    pub const MEMORY_LOAD: &str = "mem_load";
    pub const BATTERY_PERCENT: &str = "battery_percent";
    pub const BATTERY_STATE: &str = "battery_state";
    pub const SWITCH_ID: &str = "switch_id";
    pub const ENDPOINT_ID: &str = "endpoint_id";
    pub const VIA_SWITCH_ID: &str = "via_switch_id";
    pub const BLOCKED_IP_LIST: &str = "blocked_ip_list";
}

pub mod transport {
    pub const IDENTITY: &str = "transport_id";
    pub const NETWORK_IDENTITY: &str = "network_id";
    pub const UPTIME: &str = "transport_uptime";
    pub const PROBE_RESPONDER: &str = "probe_responder";
    pub const SOFTWARE_VERSION: &str = "software_version";
}

pub mod blackhole {
    pub const SOURCE: &str = "source";
}

pub mod remote_path {
    pub const TABLE: &str = "table";
    pub const RATES: &str = "rates";
}
