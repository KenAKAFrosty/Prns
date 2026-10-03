use core::num::NonZeroU64;

pub const MAX_DEVICE_LABEL_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLabel(heapless::String<MAX_DEVICE_LABEL_BYTES>);

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceLabelError {
    Blank,
    TooLong { bytes: usize, maximum: usize },
}

impl DeviceLabel {
    pub fn new(text: &str) -> Result<Self, DeviceLabelError> {
        if text.trim().is_empty() {
            return Err(DeviceLabelError::Blank);
        }
        let label = heapless::String::try_from(text).map_err(|()| DeviceLabelError::TooLong {
            bytes: text.len(),
            maximum: MAX_DEVICE_LABEL_BYTES,
        })?;
        Ok(Self(label))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceId(pub(crate) NonZeroU64);

#[cfg(test)]
mod behavior;
#[cfg(kani)]
mod proofs;
#[cfg(test)]
mod tests;
