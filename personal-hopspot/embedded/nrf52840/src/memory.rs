use personal_hopspot_memory::{JournalLayout, MemoryProfile, ProcessorArchitecture, RegionRole};
use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};

pub(crate) struct NrfFirmwareMemory {
    profile: &'static MemoryProfile,
}

impl NrfFirmwareMemory {
    pub(crate) const fn new(profile: &'static MemoryProfile) -> Self {
        assert!(matches!(
            profile.architecture,
            ProcessorArchitecture::ThumbV7em
        ));
        Self { profile }
    }

    pub(crate) const fn flash_offset(&self, role: RegionRole) -> u32 {
        narrow_address(self.region(role).range.start())
    }

    #[cfg(any(
        feature = "board-t-echo",
        feature = "board-t096",
        feature = "board-t114",
        feature = "board-mesh-pocket"
    ))]
    pub(crate) const fn two_flash_pages(&self, role: RegionRole) -> [u32; 2] {
        let region = self.region(role);
        let page_bytes = self.journal().page_bytes;
        assert!(region.range.byte_len() == 2 * page_bytes);
        [
            narrow_address(region.range.start()),
            narrow_address(region.range.start() + page_bytes),
        ]
    }

    pub(crate) const fn journal_layout(&self) -> FlashJournalLayout {
        let journal = self.journal();
        FlashJournalLayout::new(
            [
                narrow_address(journal.timebase_pages[0].start()),
                narrow_address(journal.timebase_pages[1].start()),
            ],
            [
                FlashArenaRange::new(
                    narrow_address(journal.arenas[0].start()),
                    narrow_address(journal.arenas[0].end()),
                ),
                FlashArenaRange::new(
                    narrow_address(journal.arenas[1].start()),
                    narrow_address(journal.arenas[1].end()),
                ),
            ],
        )
    }

    const fn region(&self, role: RegionRole) -> &personal_hopspot_memory::MemoryRegion {
        match self.profile.unique_region_for_role(role) {
            Ok(region) => region,
            Err(_) => panic!(),
        }
    }

    const fn journal(&self) -> &'static JournalLayout {
        assert!(self.profile.journals.len() == 1);
        &self.profile.journals[0]
    }
}

const fn narrow_address(address: u64) -> u32 {
    assert!(address <= u32::MAX as u64);
    address as u32
}
