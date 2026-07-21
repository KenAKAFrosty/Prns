use crate::interfaces::rns_serial_framing;
use crate::interfaces::MAX_WIRE_FRAME_LEN;

pub const DEFAULT_LOCAL_PORT: u16 = 37428;
pub const DEFAULT_SOCKET_PATH: &str = "default";
pub const FRAME_CAP: usize = MAX_WIRE_FRAME_LEN;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_CAP);
pub const READ_BUF_LEN: usize = FRAMED_LEN;
