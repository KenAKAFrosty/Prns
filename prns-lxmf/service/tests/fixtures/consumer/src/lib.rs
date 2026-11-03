#![forbid(unsafe_code)]

#[cfg(feature = "host")]
pub use prns_lxmf::direct::PrnsDirectNetwork;
#[cfg(feature = "runtime")]
pub use prns_lxmf::direct::{DirectLimits, DirectNetwork};
pub use prns_lxmf::DirectSendFailure;
