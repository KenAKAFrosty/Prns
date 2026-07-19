use alloc::vec::Vec;
use core::convert::Infallible;

use rmp::encode::{self, ByteBuf, ValueWriteError};

use super::MessagePackEncodeError;

pub(crate) struct MessagePackEncoder {
    bytes: ByteBuf,
}

impl MessagePackEncoder {
    pub(crate) fn new() -> Self {
        Self {
            bytes: ByteBuf::new(),
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes.into_vec()
    }

    pub(crate) fn nil(&mut self) {
        infallible(encode::write_nil(&mut self.bytes));
    }

    pub(crate) fn boolean(&mut self, value: bool) {
        infallible(encode::write_bool(&mut self.bytes, value));
    }

    pub(crate) fn signed(&mut self, value: i64) {
        infallible_value(encode::write_sint(&mut self.bytes, value));
    }

    pub(crate) fn unsigned(&mut self, value: u64) {
        infallible_value(encode::write_uint(&mut self.bytes, value));
    }

    pub(crate) fn float(&mut self, value: f64) {
        infallible_value(encode::write_f64(&mut self.bytes, value));
    }

    pub(crate) fn string(&mut self, value: &str) -> Result<(), MessagePackEncodeError> {
        u32::try_from(value.len()).map_err(|_| MessagePackEncodeError)?;
        infallible_value(encode::write_str(&mut self.bytes, value));
        Ok(())
    }

    pub(crate) fn binary(&mut self, value: &[u8]) -> Result<(), MessagePackEncodeError> {
        u32::try_from(value.len()).map_err(|_| MessagePackEncodeError)?;
        infallible_value(encode::write_bin(&mut self.bytes, value));
        Ok(())
    }

    pub(crate) fn array(&mut self, length: usize) -> Result<(), MessagePackEncodeError> {
        let length = u32::try_from(length).map_err(|_| MessagePackEncodeError)?;
        infallible_value(encode::write_array_len(&mut self.bytes, length));
        Ok(())
    }

    pub(crate) fn map(&mut self, length: usize) -> Result<(), MessagePackEncodeError> {
        let length = u32::try_from(length).map_err(|_| MessagePackEncodeError)?;
        infallible_value(encode::write_map_len(&mut self.bytes, length));
        Ok(())
    }

    #[cfg(feature = "shared-instance-rpc")]
    pub(crate) fn field(&mut self, name: &str) -> Result<(), MessagePackEncodeError> {
        self.string(name)
    }

    #[cfg(feature = "shared-instance-rpc")]
    pub(crate) fn string_field(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), MessagePackEncodeError> {
        self.field(name)?;
        self.string(value)
    }

    #[cfg(feature = "shared-instance-rpc")]
    pub(crate) fn unsigned_field(
        &mut self,
        name: &str,
        value: u64,
    ) -> Result<(), MessagePackEncodeError> {
        self.field(name)?;
        self.unsigned(value);
        Ok(())
    }
}

fn infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

fn infallible_value<T>(result: Result<T, ValueWriteError<Infallible>>) -> T {
    match result {
        Ok(value) => value,
        Err(ValueWriteError::InvalidMarkerWrite(never))
        | Err(ValueWriteError::InvalidDataWrite(never)) => match never {},
    }
}
