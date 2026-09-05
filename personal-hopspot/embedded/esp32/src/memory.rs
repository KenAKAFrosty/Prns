use personal_hopspot_memory::{
    AddressRange, AddressSpaceGeometry, AddressSpaceKind, JournalLayout, MemoryProfile,
    ProcessorArchitecture, RegionRole,
};
use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};

pub(crate) struct EspFirmwareMemory {
    profile: &'static MemoryProfile,
}

impl EspFirmwareMemory {
    pub(crate) const fn new(profile: &'static MemoryProfile) -> Self {
        #[cfg(target_arch = "xtensa")]
        assert!(matches!(
            profile.architecture,
            ProcessorArchitecture::XtensaEsp32S3
        ));
        #[cfg(target_arch = "riscv32")]
        assert!(matches!(
            profile.architecture,
            ProcessorArchitecture::RiscV32Imac
        ));
        Self { profile }
    }

    pub(crate) const fn flash_capacity(&self) -> usize {
        let flash = self.internal_flash();
        assert!(flash.start() == 0);
        narrow_size(flash.end())
    }

    pub(crate) const fn identity_flash_capacity(&self) -> usize {
        narrow_size(self.region(RegionRole::NodeIdentity).range.end())
    }

    pub(crate) const fn ble_identity_offset(&self) -> u32 {
        self.flash_offset(RegionRole::BleIdentity)
    }

    pub(crate) const fn node_identity_offset(&self) -> u32 {
        self.flash_offset(RegionRole::NodeIdentity)
    }

    pub(crate) const fn remote_control_identity_offset(&self) -> u32 {
        self.flash_offset(RegionRole::RemoteControlIdentity)
    }

    #[cfg(target_arch = "xtensa")]
    pub(crate) const fn radio_profile_pages(&self) -> [u32; 2] {
        let region = self.region(RegionRole::RadioProfile);
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

    pub(crate) const fn journal_supports(&self, required_arena_bytes: usize) -> bool {
        let journal = self.journal();
        journal.arenas[0].byte_len() >= required_arena_bytes as u64
            && journal.arenas[1].byte_len() >= required_arena_bytes as u64
    }

    const fn flash_offset(&self, role: RegionRole) -> u32 {
        narrow_address(self.region(role).range.start())
    }

    const fn internal_flash(&self) -> AddressRange {
        let address_space = match self
            .profile
            .unique_address_space_for_kind(AddressSpaceKind::InternalFlash)
        {
            Ok(address_space) => address_space,
            Err(_) => panic!(),
        };
        match address_space.geometry {
            AddressSpaceGeometry::Fixed(range) => range,
            AddressSpaceGeometry::FixedCapacity { .. }
            | AddressSpaceGeometry::LinkerDefined
            | AddressSpaceGeometry::RuntimeDetected => panic!(),
        }
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

const fn narrow_size(bytes: u64) -> usize {
    assert!(bytes <= usize::MAX as u64);
    bytes as usize
}
