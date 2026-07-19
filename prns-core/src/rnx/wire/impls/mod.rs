mod owned;
#[cfg(test)]
mod tests;

pub use owned::{
    decode_execution_request, decode_execution_result, encode_execution_request,
    encode_execution_result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnxCodecError {
    MessagePack(crate::message_pack::MessagePackOwnedError),
    MalformedMessagePack,
    BufferTooShort,
    ExpectedArray,
    WrongFieldCount,
    InvalidField(super::RnxField),
    InvalidUtf8,
    IncoherentResult,
}
