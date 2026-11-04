use core::{fmt, num::NonZeroU64};

/// A device endpoint identity, distinct from an account or logical group address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EndpointId(pub [u8; 32]);

/// A never-reused incarnation of an endpoint. Enrollment owns its allocation;
/// the core checks nonzero representation, not historical uniqueness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EndpointIncarnation(NonZeroU64);

impl EndpointIncarnation {
    pub const fn new(value: u64) -> Result<Self, EndpointError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(EndpointError::ZeroIncarnation),
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupEndpoint"))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Endpoint {
    pub id: EndpointId,
    pub incarnation: EndpointIncarnation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    ZeroIncarnation,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid endpoint: {self:?}")
    }
}

impl core::error::Error for EndpointError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_not_an_endpoint_incarnation() {
        assert_eq!(
            EndpointIncarnation::new(0),
            Err(EndpointError::ZeroIncarnation)
        );
        assert_eq!(EndpointIncarnation::new(1).map(|value| value.get()), Ok(1));
        assert_eq!(
            EndpointIncarnation::new(u64::MAX).map(|value| value.get()),
            Ok(u64::MAX)
        );
    }
}
