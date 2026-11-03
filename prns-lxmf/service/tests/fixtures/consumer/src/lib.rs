#![forbid(unsafe_code)]

#[cfg(feature = "host")]
pub use prns_lxmf::direct::PrnsDirectNetwork;
#[cfg(feature = "runtime")]
pub use prns_lxmf::direct::{DirectLimits, DirectNetwork};
#[cfg(all(feature = "runtime", feature = "mailbox"))]
pub use prns_lxmf::mailbox::ReplayLimits;
#[cfg(feature = "mailbox")]
pub use prns_lxmf::mailbox::{MailboxLimits, MailboxRequest};
pub use prns_lxmf::DirectSendFailure;
