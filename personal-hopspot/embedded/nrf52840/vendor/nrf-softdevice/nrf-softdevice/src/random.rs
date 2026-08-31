use crate::{raw, RawError, Softdevice};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RandomError {
    BufferTooBig,
    NotEnoughEntropy,
    Raw(RawError),
}

/// A movable capability for the enabled SoftDevice's application RNG pool.
pub struct SoftdeviceRandom {
    _private: (),
}

impl From<RawError> for RandomError {
    fn from(err: RawError) -> Self {
        Self::Raw(err)
    }
}

impl Softdevice {
    /// Issues access to the application RNG pool after the SoftDevice is enabled.
    pub fn random(&self) -> SoftdeviceRandom {
        SoftdeviceRandom { _private: () }
    }
}

impl SoftdeviceRandom {
    /// Get cryptographically secure random bytes from the enabled SoftDevice.
    pub fn random_bytes(&mut self, buf: &mut [u8]) -> Result<(), RandomError> {
        if buf.len() > u8::MAX as usize {
            return Err(RandomError::BufferTooBig);
        }

        let ret = unsafe {
            raw::sd_rand_application_vector_get(buf[..].as_mut_ptr(), buf.len() as u8)
        };
        match RawError::convert(ret) {
            Ok(()) => Ok(()),
            Err(RawError::SocRandNotEnoughValues) => Err(RandomError::NotEnoughEntropy),
            Err(e) => Err(e.into()),
        }
    }
}

/// Get cryptographically secure random bytes.
pub fn random_bytes(sd: &Softdevice, buf: &mut [u8]) -> Result<(), RandomError> {
    sd.random().random_bytes(buf)
}
