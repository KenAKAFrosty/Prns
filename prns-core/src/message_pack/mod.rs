mod decoder;
#[cfg(any(
    feature = "rnx",
    feature = "shared-instance-rpc",
    feature = "signed-artifact"
))]
mod encoder;
#[cfg(any(feature = "rnx", feature = "signed-artifact"))]
mod owned;

pub(crate) use decoder::{Marker, MessagePackInteger, MessagePackReader};
#[cfg(any(
    feature = "rnx",
    feature = "shared-instance-rpc",
    feature = "signed-artifact"
))]
pub(crate) use encoder::MessagePackEncoder;
#[cfg(any(feature = "rnx", feature = "signed-artifact"))]
pub use owned::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackDecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackEncodeError;
