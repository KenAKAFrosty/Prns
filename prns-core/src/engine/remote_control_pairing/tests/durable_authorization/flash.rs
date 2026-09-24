use crate::persistence::{FlashArenaRange, FlashJournalLayout};
use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
use embedded_storage_async::nor_flash::{NorFlash, ReadNorFlash};

const PAGE: usize = 1024;
const CAPACITY: usize = PAGE * 4;
pub(super) const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0, PAGE as u32],
    [
        FlashArenaRange::new((PAGE * 2) as u32, (PAGE * 3) as u32),
        FlashArenaRange::new((PAGE * 3) as u32, CAPACITY as u32),
    ],
);

#[derive(Debug)]
pub(super) struct PowerLost;

impl NorFlashError for PowerLost {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

pub(super) struct Flash {
    bytes: [u8; CAPACITY],
    programmed: usize,
    cut: Option<usize>,
    powered: bool,
}

impl Flash {
    pub(super) fn new() -> Self {
        Self {
            bytes: [0xff; CAPACITY],
            programmed: 0,
            cut: None,
            powered: true,
        }
    }

    pub(super) fn reboot(&self) -> Self {
        Self {
            bytes: self.bytes,
            programmed: 0,
            cut: None,
            powered: true,
        }
    }

    pub(super) fn cut_after(&self, bytes: usize) -> Self {
        Self {
            cut: Some(bytes),
            ..self.reboot()
        }
    }

    pub(super) fn programmed_bytes(&self) -> usize {
        self.programmed
    }
}

impl ErrorType for Flash {
    type Error = PowerLost;
}

impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;
    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), PowerLost> {
        if !self.powered {
            return Err(PowerLost);
        }
        let start = offset as usize;
        assert!(start.is_multiple_of(Self::READ_SIZE));
        assert!(bytes.len().is_multiple_of(Self::READ_SIZE));
        bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
        Ok(())
    }
    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = PAGE;
    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), PowerLost> {
        if !self.powered {
            return Err(PowerLost);
        }
        let start = offset as usize;
        assert!(start.is_multiple_of(Self::WRITE_SIZE));
        assert!(bytes.len().is_multiple_of(Self::WRITE_SIZE));
        for (index, byte) in bytes.iter().enumerate() {
            if self.cut == Some(self.programmed) {
                self.powered = false;
                return Err(PowerLost);
            }
            let slot = &mut self.bytes[start + index];
            assert_eq!(*slot, 0xff, "the journal must not reprogram occupied cells");
            *slot &= byte;
            self.programmed += 1;
        }
        if self.cut == Some(self.programmed) {
            self.powered = false;
            return Err(PowerLost);
        }
        Ok(())
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), PowerLost> {
        assert!(
            self.cut.is_none(),
            "this campaign cuts appends, not erase operations"
        );
        assert!((from as usize).is_multiple_of(PAGE));
        assert!((to as usize).is_multiple_of(PAGE));
        self.bytes[from as usize..to as usize].fill(0xff);
        Ok(())
    }
}
