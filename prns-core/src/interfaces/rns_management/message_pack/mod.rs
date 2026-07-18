mod decoder;
mod encoder;

pub(super) use decoder::{MessagePackInteger, MessagePackReader};
pub(crate) use encoder::MessagePackEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MessagePackDecodeError;
