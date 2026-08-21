pub(crate) mod section {
    pub const RETICULUM: &str = "reticulum";
    pub const PRNS: &str = "prns";
    pub const LOGGING: &str = "logging";
    pub const INTERFACES: &str = "interfaces";
}

pub(crate) mod prns {
    pub const RESOURCE_MEM_IN: &str = "resource_mem_in";
    pub const RESOURCE_MEM_OUT: &str = "resource_mem_out";
}

pub(crate) mod global {
    pub const SHARE_INSTANCE: &str = "share_instance";
    pub const INSTANCE_NAME: &str = "instance_name";
    pub const SHARED_INSTANCE_TYPE: &str = "shared_instance_type";
    pub const SHARED_INSTANCE_PORT: &str = "shared_instance_port";
    pub const INSTANCE_CONTROL_PORT: &str = "instance_control_port";
    pub const RPC_KEY: &str = "rpc_key";
    pub const ENABLE_TRANSPORT: &str = "enable_transport";
    pub const STATIC_TRANSPORT_IDENTITY: &str = "static_transport_identity";
    pub const LOCAL_HOPS_DELTA: &str = "local_hops_delta";
    pub const NETWORK_IDENTITY: &str = "network_identity";
    pub const LINK_MTU_DISCOVERY: &str = "link_mtu_discovery";
    pub const ENABLE_REMOTE_MANAGEMENT: &str = "enable_remote_management";
    pub const REMOTE_MANAGEMENT_ALLOWED: &str = "remote_management_allowed";
    pub const RESPOND_TO_PROBES: &str = "respond_to_probes";
    pub const FORCE_SHARED_INSTANCE_BITRATE: &str = "force_shared_instance_bitrate";
    pub const PANIC_ON_INTERFACE_ERROR: &str = "panic_on_interface_error";
    pub const USE_IMPLICIT_PROOF: &str = "use_implicit_proof";
    pub const DISCOVER_INTERFACES: &str = "discover_interfaces";
    pub const REQUIRED_DISCOVERY_VALUE: &str = "required_discovery_value";
    pub const PUBLISH_BLACKHOLE: &str = "publish_blackhole";
    pub const BLACKHOLE_SOURCES: &str = "blackhole_sources";
    pub const BLACKHOLE_UPDATE_INTERVAL: &str = "blackhole_update_interval";
    pub const INTERFACE_DISCOVERY_SOURCES: &str = "interface_discovery_sources";
    pub const AUTOCONNECT_DISCOVERED_INTERFACES: &str = "autoconnect_discovered_interfaces";
    pub const AUTOCONNECT_INTERFACE_GRAVITY: &str = "autoconnect_interface_gravity";
    pub const AUTOCONNECT_ANNOUNCES_TO_INTERNAL: &str = "autoconnect_announces_to_internal";
    pub const DEFAULT_GRAVITY: &str = "default_gravity";
    pub const DEFAULT_AR_TARGET: &str = "default_ar_target";
    pub const DEFAULT_AR_PENALTY: &str = "default_ar_penalty";
    pub const DEFAULT_AR_GRACE: &str = "default_ar_grace";
}

pub(crate) mod logging {
    pub const LEVEL: &str = "loglevel";
    pub const TIMESTAMPS: &str = "logtimestamps";
}

pub(crate) mod rnode {
    pub const TCP_SCHEME: &str = "tcp://";
    pub const BLE_SCHEME: &str = "ble://";
}

pub(crate) mod common {
    pub const INGRESS_CONTROL: &str = "ingress_control";
    pub const EGRESS_CONTROL: &str = "egress_control";
    pub const IC_MAX_HELD_ANNOUNCES: &str = "ic_max_held_announces";
    pub const IC_BURST_HOLD: &str = "ic_burst_hold";
    pub const IC_BURST_FREQ_NEW: &str = "ic_burst_freq_new";
    pub const IC_BURST_FREQ: &str = "ic_burst_freq";
    pub const IC_PR_BURST_FREQ_NEW: &str = "ic_pr_burst_freq_new";
    pub const IC_PR_BURST_FREQ: &str = "ic_pr_burst_freq";
    pub const EC_PR_FREQ: &str = "ec_pr_freq";
    pub const IC_NEW_TIME: &str = "ic_new_time";
    pub const IC_BURST_PENALTY: &str = "ic_burst_penalty";
    pub const IC_HELD_RELEASE_INTERVAL: &str = "ic_held_release_interval";
}

pub(crate) mod interface {
    use super::common;

    pub const TYPE: &str = "type";
    pub const INTERFACE_ENABLED: &str = "interface_enabled";
    pub const ENABLED: &str = "enabled";
    pub const INTERFACE_MODE: &str = "interface_mode";
    pub const MODE: &str = "mode";
    pub const OUTGOING: &str = "outgoing";
    pub const BITRATE: &str = "bitrate";
    pub const GRAVITY: &str = "gravity";
    pub const ANNOUNCE_CAP: &str = "announce_cap";
    pub const ANNOUNCE_RATE_TARGET: &str = "announce_rate_target";
    pub const ANNOUNCE_RATE_GRACE: &str = "announce_rate_grace";
    pub const ANNOUNCE_RATE_PENALTY: &str = "announce_rate_penalty";
    pub const NETWORK_NAME: &str = "network_name";
    pub const NETWORKNAME: &str = "networkname";
    pub const PASS_PHRASE: &str = "pass_phrase";
    pub const PASSPHRASE: &str = "passphrase";
    pub const IFAC_SIZE: &str = "ifac_size";
    pub const DISCOVERABLE: &str = "discoverable";
    pub const ANNOUNCE_INTERVAL: &str = "announce_interval";
    pub const DISCOVERY_STAMP_VALUE: &str = "discovery_stamp_value";
    pub const DISCOVERY_NAME: &str = "discovery_name";
    pub const DISCOVERY_ENCRYPT: &str = "discovery_encrypt";
    pub const REACHABLE_ON: &str = "reachable_on";
    pub const REACHABLE_PORT: &str = "reachable_port";
    pub const PUBLISH_IFAC: &str = "publish_ifac";
    pub const LATITUDE: &str = "latitude";
    pub const LONGITUDE: &str = "longitude";
    pub const HEIGHT: &str = "height";
    pub const DISCOVERY_FREQUENCY: &str = "discovery_frequency";
    pub const DISCOVERY_BANDWIDTH: &str = "discovery_bandwidth";
    pub const DISCOVERY_MODULATION: &str = "discovery_modulation";
    pub const BOOTSTRAP_ONLY: &str = "bootstrap_only";
    pub const RECURSIVE_PRS: &str = "recursive_prs";
    pub const ANNOUNCES_FROM_INTERNAL: &str = "announces_from_internal";
    pub const ANNOUNCES_TO_INTERNAL: &str = "announces_to_internal";
    pub const IGNORE_CONFIG_WARNINGS: &str = "ignore_config_warnings";
    pub const GROUP_ID: &str = "group_id";
    pub const DISCOVERY_SCOPE: &str = "discovery_scope";
    pub const DISCOVERY_PORT: &str = "discovery_port";
    pub const DATA_PORT: &str = "data_port";
    pub const DEVICES: &str = "devices";
    pub const IGNORED_DEVICES: &str = "ignored_devices";
    pub const MULTICAST_ADDRESS_TYPE: &str = "multicast_address_type";
    pub const TARGET_HOST: &str = "target_host";
    pub const TARGET_PORT: &str = "target_port";
    pub const TARGET: &str = "target";
    pub const FRAMING: &str = "framing";
    pub const KISS_FRAMING: &str = "kiss_framing";
    pub const I2P_TUNNELED: &str = "i2p_tunneled";
    pub const CONNECT_TIMEOUT: &str = "connect_timeout";
    pub const MAX_RECONNECT_TRIES: &str = "max_reconnect_tries";
    pub const FIXED_MTU: &str = "fixed_mtu";
    pub const LISTEN_IP: &str = "listen_ip";
    pub const LISTEN_PORT: &str = "listen_port";
    pub const DEVICE: &str = "device";
    pub const PORT: &str = "port";
    pub const PREFER_IPV6: &str = "prefer_ipv6";
    pub const FORWARD_IP: &str = "forward_ip";
    pub const FORWARD_PORT: &str = "forward_port";
    pub const SPEED: &str = "speed";
    pub const DATABITS: &str = "databits";
    pub const PARITY: &str = "parity";
    pub const STOPBITS: &str = "stopbits";
    pub const FLOW_CONTROL: &str = "flow_control";
    pub const PREAMBLE: &str = "preamble";
    pub const TXTAIL: &str = "txtail";
    pub const PERSISTENCE: &str = "persistence";
    pub const SLOTTIME: &str = "slottime";
    pub const ID_CALLSIGN: &str = "id_callsign";
    pub const ID_INTERVAL: &str = "id_interval";
    pub const CALLSIGN: &str = "callsign";
    pub const SSID: &str = "ssid";
    pub const FREQUENCY: &str = "frequency";
    pub const BANDWIDTH: &str = "bandwidth";
    pub const SPREADINGFACTOR: &str = "spreadingfactor";
    pub const CODINGRATE: &str = "codingrate";
    pub const TXPOWER: &str = "txpower";
    pub const AIRTIME_LIMIT_SHORT: &str = "airtime_limit_short";
    pub const AIRTIME_LIMIT_LONG: &str = "airtime_limit_long";
    pub const COMMAND: &str = "command";
    pub const RESPAWN_DELAY: &str = "respawn_delay";
    pub const REMOTE: &str = "remote";
    pub const LISTEN_ON: &str = "listen_on";
    pub const VPORT: &str = "vport";
    pub const PEERS: &str = "peers";
    pub const CONNECTABLE: &str = "connectable";

    pub const ENABLED_ALIASES: &[&str] = &[INTERFACE_ENABLED, ENABLED];
    pub const MODE_ALIASES: &[&str] = &[INTERFACE_MODE, MODE];
    pub const NETWORK_NAME_ALIASES: &[&str] = &[NETWORK_NAME, NETWORKNAME];
    pub const PASSPHRASE_ALIASES: &[&str] = &[PASS_PHRASE, PASSPHRASE];
    pub const ALIASES: &[&str] = &[
        INTERFACE_ENABLED,
        ENABLED,
        INTERFACE_MODE,
        MODE,
        NETWORK_NAME,
        NETWORKNAME,
        PASS_PHRASE,
        PASSPHRASE,
    ];

    pub const COMMON: &[&str] = &[
        TYPE,
        INTERFACE_ENABLED,
        ENABLED,
        INTERFACE_MODE,
        MODE,
        OUTGOING,
        BITRATE,
        GRAVITY,
        ANNOUNCE_CAP,
        ANNOUNCE_RATE_TARGET,
        ANNOUNCE_RATE_GRACE,
        ANNOUNCE_RATE_PENALTY,
        NETWORK_NAME,
        NETWORKNAME,
        PASS_PHRASE,
        PASSPHRASE,
        IFAC_SIZE,
        DISCOVERABLE,
        ANNOUNCE_INTERVAL,
        DISCOVERY_STAMP_VALUE,
        DISCOVERY_NAME,
        DISCOVERY_ENCRYPT,
        REACHABLE_ON,
        REACHABLE_PORT,
        PUBLISH_IFAC,
        LATITUDE,
        LONGITUDE,
        HEIGHT,
        DISCOVERY_FREQUENCY,
        DISCOVERY_BANDWIDTH,
        DISCOVERY_MODULATION,
        common::INGRESS_CONTROL,
        common::EGRESS_CONTROL,
        common::IC_MAX_HELD_ANNOUNCES,
        common::IC_BURST_HOLD,
        common::IC_BURST_FREQ_NEW,
        common::IC_BURST_FREQ,
        common::IC_PR_BURST_FREQ_NEW,
        common::IC_PR_BURST_FREQ,
        common::EC_PR_FREQ,
        common::IC_NEW_TIME,
        common::IC_BURST_PENALTY,
        common::IC_HELD_RELEASE_INTERVAL,
        BOOTSTRAP_ONLY,
        RECURSIVE_PRS,
        ANNOUNCES_FROM_INTERNAL,
        ANNOUNCES_TO_INTERNAL,
        IGNORE_CONFIG_WARNINGS,
    ];
    pub const AUTO: &[&str] = &[
        GROUP_ID,
        DISCOVERY_SCOPE,
        DISCOVERY_PORT,
        DATA_PORT,
        DEVICES,
        IGNORED_DEVICES,
        MULTICAST_ADDRESS_TYPE,
    ];
    pub const TCP_CLIENT: &[&str] = &[
        TARGET_HOST,
        TARGET_PORT,
        KISS_FRAMING,
        I2P_TUNNELED,
        CONNECT_TIMEOUT,
        MAX_RECONNECT_TRIES,
        FIXED_MTU,
    ];
    pub const TCP_SERVER: &[&str] = &[
        LISTEN_IP,
        LISTEN_PORT,
        DEVICE,
        PORT,
        PREFER_IPV6,
        I2P_TUNNELED,
        KISS_FRAMING,
        FIXED_MTU,
    ];
    pub const UDP: &[&str] = &[
        LISTEN_IP,
        LISTEN_PORT,
        FORWARD_IP,
        FORWARD_PORT,
        DEVICE,
        PORT,
    ];
    pub const SERIAL: &[&str] = &[PORT, SPEED, DATABITS, PARITY, STOPBITS];
    pub const KISS: &[&str] = &[
        PORT,
        SPEED,
        DATABITS,
        PARITY,
        STOPBITS,
        FLOW_CONTROL,
        PREAMBLE,
        TXTAIL,
        PERSISTENCE,
        SLOTTIME,
        ID_CALLSIGN,
        ID_INTERVAL,
    ];
    pub const AX25_KISS: &[&str] = &[
        PORT,
        SPEED,
        DATABITS,
        PARITY,
        STOPBITS,
        FLOW_CONTROL,
        PREAMBLE,
        TXTAIL,
        PERSISTENCE,
        SLOTTIME,
        CALLSIGN,
        SSID,
    ];
    pub const RNODE: &[&str] = &[
        PORT,
        FREQUENCY,
        BANDWIDTH,
        SPREADINGFACTOR,
        CODINGRATE,
        TXPOWER,
        FLOW_CONTROL,
        ID_CALLSIGN,
        ID_INTERVAL,
        AIRTIME_LIMIT_SHORT,
        AIRTIME_LIMIT_LONG,
    ];
    pub const RNODE_MULTI: &[&str] = &[PORT, ID_CALLSIGN, ID_INTERVAL];
    pub const RNODE_MULTI_SUBINTERFACE: &[&str] = &[
        INTERFACE_ENABLED,
        ENABLED,
        VPORT,
        FREQUENCY,
        BANDWIDTH,
        SPREADINGFACTOR,
        CODINGRATE,
        TXPOWER,
        FLOW_CONTROL,
        AIRTIME_LIMIT_SHORT,
        AIRTIME_LIMIT_LONG,
        OUTGOING,
    ];
    pub const PIPE: &[&str] = &[COMMAND, RESPAWN_DELAY];
    pub const BACKBONE: &[&str] = &[
        LISTEN_IP,
        LISTEN_PORT,
        TARGET_HOST,
        TARGET_PORT,
        PORT,
        DEVICE,
        PREFER_IPV6,
        I2P_TUNNELED,
        CONNECT_TIMEOUT,
        MAX_RECONNECT_TRIES,
        REMOTE,
        LISTEN_ON,
    ];
    pub const I2P: &[&str] = &[PEERS, CONNECTABLE];
    pub const WEAVE: &[&str] = &[PORT];
    pub const PRNS_WEBSOCKET_CLIENT: &[&str] = &[TARGET, FRAMING];
    pub const PRNS_WEBSOCKET_SERVER: &[&str] =
        &[LISTEN_IP, LISTEN_PORT, DEVICE, PORT, PREFER_IPV6, FRAMING];
}
