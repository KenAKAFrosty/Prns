mod decoder;
mod encoder;
mod owned;

pub(crate) use decoder::{MessagePackInteger, MessagePackReader};
pub(crate) use encoder::MessagePackEncoder;
pub use owned::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackDecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackEncodeError;
