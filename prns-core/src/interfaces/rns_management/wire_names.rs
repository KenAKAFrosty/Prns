pub(crate) mod common {
    pub const HASH: &str = "hash";
    pub const UNTIL: &str = "until";
    pub const REASON: &str = "reason";
}

pub(super) mod path {
    pub const VIA: &str = "via";
    pub const HOPS: &str = "hops";
    pub const TIMESTAMP: &str = "timestamp";
    pub const EXPIRES: &str = "expires";
    pub const INTERFACE: &str = "interface";
}

pub(super) mod rate {
    pub const LAST: &str = "last";
    pub const VIOLATIONS: &str = "rate_violations";
    pub const BLOCKED_UNTIL: &str = "blocked_until";
    pub const TIMESTAMPS: &str = "timestamps";
}

pub(super) mod interface {
    pub const INTERFACES: &str = "interfaces";
    pub const NAME: &str = "name";
    pub const SHORT_NAME: &str = "short_name";
    pub const TYPE: &str = "type";
    pub const STATUS: &str = "status";
    pub const MODE: &str = "mode";
    pub const CLIENTS: &str = "clients";
    pub const RECEIVE_BYTES: &str = "rxb";
    pub const TRANSMIT_BYTES: &str = "txb";
    pub const RECEIVE_SPEED: &str = "rxs";
    pub const TRANSMIT_SPEED: &str = "txs";
    pub const IFAC_SIGNATURE: &str = "ifac_signature";
    pub const IFAC_SIZE: &str = "ifac_size";
    pub const IFAC_NETWORK_NAME: &str = "ifac_netname";
    pub const RESIDENT_SET_SIZE: &str = "rss";
}

pub(super) mod transport {
    pub const IDENTITY: &str = "transport_id";
    pub const NETWORK_IDENTITY: &str = "network_id";
    pub const UPTIME: &str = "transport_uptime";
    pub const PROBE_RESPONDER: &str = "probe_responder";
}

pub(super) mod blackhole {
    pub const SOURCE: &str = "source";
}

pub(super) mod remote_path {
    pub const TABLE: &str = "table";
    pub const RATES: &str = "rates";
}
