#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
}

#[cfg(feature = "alloc")]
pub mod bring_up;
#[cfg(feature = "alloc")]
pub mod live;
#[cfg(feature = "alloc")]
pub mod multi;
pub mod policy;
#[cfg(feature = "alloc")]
pub mod protocol;
