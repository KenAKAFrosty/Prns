use personal_hopspot_memory::{
    JournalLayout, MemoryProfile, MemoryRegion, ProcessorArchitecture, RegionRole,
};
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
        feature = "board-t114"
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

    const fn region(&self, role: RegionRole) -> &'static MemoryRegion {
        let mut matching_index = self.profile.regions.len();
        let mut index = 0;
        while index < self.profile.regions.len() {
            if same_role(self.profile.regions[index].role, role) {
                assert!(matching_index == self.profile.regions.len());
                matching_index = index;
            }
            index += 1;
        }
        assert!(matching_index < self.profile.regions.len());
        &self.profile.regions[matching_index]
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

const fn same_role(left: RegionRole, right: RegionRole) -> bool {
    matches!(
        (left, right),
        (RegionRole::Bootloader, RegionRole::Bootloader)
            | (RegionRole::PartitionTable, RegionRole::PartitionTable)
            | (RegionRole::SoftDevice, RegionRole::SoftDevice)
            | (RegionRole::PlatformData, RegionRole::PlatformData)
            | (RegionRole::FirmwareImage, RegionRole::FirmwareImage)
            | (RegionRole::BleIdentity, RegionRole::BleIdentity)
            | (RegionRole::Provisioning, RegionRole::Provisioning)
            | (RegionRole::NodeIdentity, RegionRole::NodeIdentity)
            | (RegionRole::PhyInitialization, RegionRole::PhyInitialization)
            | (
                RegionRole::RemoteControlIdentity,
                RegionRole::RemoteControlIdentity
            )
            | (RegionRole::RadioProfile, RegionRole::RadioProfile)
            | (RegionRole::Journal, RegionRole::Journal)
            | (
                RegionRole::RecoveryBootloader,
                RegionRole::RecoveryBootloader
            )
            | (RegionRole::FactoryReserved, RegionRole::FactoryReserved)
            | (RegionRole::Reserved, RegionRole::Reserved)
    )
}
