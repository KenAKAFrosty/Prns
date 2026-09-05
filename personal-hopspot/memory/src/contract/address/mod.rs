#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressSpaceId(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackingStoreId(pub &'static str);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressRange {
    start: u64,
    end: u64,
}

impl AddressRange {
    #[must_use]
    pub const fn new(start: u64, end: u64) -> Self {
        assert!(start < end, "address ranges must be non-empty");
        Self { start, end }
    }

    pub const fn from_start_and_size(start: u64, bytes: u64) -> Result<Self, AddressRangeError> {
        if bytes == 0 {
            return Err(AddressRangeError::Empty);
        }
        match start.checked_add(bytes) {
            Some(end) => Ok(Self { start, end }),
            None => Err(AddressRangeError::Overflow),
        }
    }

    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }

    #[must_use]
    pub const fn byte_len(self) -> u64 {
        self.end - self.start
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    #[must_use]
    pub const fn is_aligned(self, alignment: Alignment) -> bool {
        self.start.is_multiple_of(alignment.bytes) && self.end.is_multiple_of(alignment.bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressRangeError {
    Empty,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alignment {
    bytes: u64,
}

impl Alignment {
    pub const fn try_new(bytes: u64) -> Result<Self, AlignmentError> {
        if bytes == 0 {
            Err(AlignmentError::Zero)
        } else if !bytes.is_power_of_two() {
            Err(AlignmentError::NotPowerOfTwo { bytes })
        } else {
            Ok(Self { bytes })
        }
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentError {
    Zero,
    NotPowerOfTwo { bytes: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceKind {
    InternalFlash,
    InternalRam,
    InstructionRam,
    DataRam,
    ReclaimedRam,
    DataCacheRam,
    ExternalPsram,
    ExternalStorage,
}

impl AddressSpaceKind {
    pub(super) const fn is_ram(self) -> bool {
        matches!(
            self,
            Self::InternalRam
                | Self::InstructionRam
                | Self::DataRam
                | Self::ReclaimedRam
                | Self::DataCacheRam
                | Self::ExternalPsram
        )
    }

    pub(super) const fn is_external(self) -> bool {
        matches!(self, Self::ExternalPsram | Self::ExternalStorage)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceGeometry {
    Fixed(AddressRange),
    FixedCapacity { bytes: u64 },
    LinkerDefined,
    RuntimeDetected,
}

impl AddressSpaceGeometry {
    pub(super) const fn contains(self, range: AddressRange) -> bool {
        match self {
            Self::Fixed(bounds) => bounds.contains(range),
            Self::FixedCapacity { bytes } => range.end <= bytes,
            Self::LinkerDefined | Self::RuntimeDetected => true,
        }
    }

    pub(super) const fn fixed_capacity(self) -> Option<u64> {
        match self {
            Self::Fixed(bounds) => Some(bounds.byte_len()),
            Self::FixedCapacity { bytes } => Some(bytes),
            Self::LinkerDefined | Self::RuntimeDetected => None,
        }
    }

    pub(super) const fn physical_offset(self, address: u64) -> Option<u64> {
        match self {
            Self::Fixed(bounds) => address.checked_sub(bounds.start),
            Self::FixedCapacity { .. } => Some(address),
            Self::LinkerDefined | Self::RuntimeDetected => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressSpace {
    pub id: AddressSpaceId,
    pub kind: AddressSpaceKind,
    pub geometry: AddressSpaceGeometry,
    pub backing_store: BackingStoreId,
    pub backing_offset: u64,
}

#[cfg(test)]
mod tests;
