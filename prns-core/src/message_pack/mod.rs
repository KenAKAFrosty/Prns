mod decoder;
#[cfg(feature = "alloc")]
mod encoder;
#[cfg(feature = "alloc")]
mod owned;

pub(crate) use decoder::{MessagePackInteger, MessagePackReader};
#[cfg(feature = "alloc")]
pub(crate) use encoder::MessagePackEncoder;
#[cfg(feature = "alloc")]
pub use owned::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackDecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackEncodeError;
