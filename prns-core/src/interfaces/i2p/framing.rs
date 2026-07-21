use crate::interfaces::rns_serial_framing;

use super::I2P_HW_MTU;

pub const READ_BUF_LEN: usize = 4_096;
pub const FRAME_LEN: usize = I2P_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_LEN);
