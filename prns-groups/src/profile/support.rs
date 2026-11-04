//! Local carrier capabilities, independent of the authenticated group profile.
use crate::{enrollment::EnrollmentError, ApiError, GroupProfile};

/// Payload ceilings projected by the selected carrier's own framing rules.
/// This native-only value describes local support, not a negotiated group limit
/// or attached hardware. Local-only owners need not select it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileSupport {
    max_wire_bytes: usize,
    max_enrollment_frame_bytes: usize,
}

impl ProfileSupport {
    pub fn new(max_wire_bytes: usize, max_enrollment_frame_bytes: usize) -> Result<Self, ApiError> {
        if max_wire_bytes == 0 || max_enrollment_frame_bytes == 0 {
            return Err(ApiError::InvalidInput);
        }
        Ok(Self {
            max_wire_bytes,
            max_enrollment_frame_bytes,
        })
    }

    pub(crate) fn intersect(self, other: Self) -> Self {
        Self {
            max_wire_bytes: self.max_wire_bytes.min(other.max_wire_bytes),
            max_enrollment_frame_bytes: self
                .max_enrollment_frame_bytes
                .min(other.max_enrollment_frame_bytes),
        }
    }

    pub(crate) fn require_wire(self, bytes: usize) -> Result<(), EnrollmentError> {
        if bytes > self.max_wire_bytes {
            return Err(EnrollmentError::UnsupportedProfile);
        }
        Ok(())
    }

    pub(crate) fn require(self, profile: GroupProfile) -> Result<(), EnrollmentError> {
        let wire = usize::try_from(profile.max_wire_bytes)
            .map_err(|_| EnrollmentError::UnsupportedProfile)?;
        let enrollment = usize::try_from(profile.max_enrollment_frame_bytes)
            .map_err(|_| EnrollmentError::UnsupportedProfile)?;
        self.require_wire(wire)?;
        if enrollment > self.max_enrollment_frame_bytes {
            return Err(EnrollmentError::UnsupportedProfile);
        }
        Ok(())
    }
}
