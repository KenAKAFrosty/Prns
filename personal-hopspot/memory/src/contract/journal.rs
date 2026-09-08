use super::{AddressRange, MemoryRegionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalLayout {
    pub region: MemoryRegionId,
    pub page_bytes: u64,
    pub timebase_pages: [AddressRange; 2],
    pub arenas: [AddressRange; 2],
}
