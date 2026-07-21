use crate::interfaces::rns_serial_framing;

pub const PEERING_TIMEOUT_MILLIS: u64 = 20_000;
pub const HANDSHAKE_TIMEOUT_MILLIS: u64 = 2_000;
pub const MULTIPATH_DEDUPLICATION_MILLIS: u64 = 750;
pub const MULTIPATH_DEDUPLICATION_CAPACITY: usize = 48;
pub const READ_BUF_LEN: usize = 1_500;
pub const WDCL_MAX_CHUNK: usize = 32_768;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(WDCL_MAX_CHUNK);
