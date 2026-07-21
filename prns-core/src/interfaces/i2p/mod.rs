mod framing;
mod liveness;
mod policy;

#[cfg(feature = "i2p")]
pub mod sam;

pub use framing::{FRAMED_LEN, FRAME_LEN, READ_BUF_LEN};
pub use liveness::{
    I2pIdleWatchdog, I2pReadObservation, I2pWatchdogVerdict, HDLC_KEEPALIVE, KEEPALIVE_AFTER,
    READ_TIMEOUT, STALE_AFTER, WATCHDOG_TICK_INTERVAL,
};
pub use policy::{configured_policy, descriptor, DEFAULTS, I2P_BITRATE_ESTIMATE, I2P_HW_MTU};
