#[cfg(feature = "shared-instance-rpc")]
pub(super) use crate::message_pack::MessagePackDecodeError;
#[cfg(feature = "shared-instance-rpc")]
pub(crate) use crate::message_pack::MessagePackEncoder;
pub(super) use crate::message_pack::{MessagePackInteger, MessagePackReader};
