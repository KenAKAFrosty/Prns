use embedded_storage_async::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};
use std::cell::RefCell;
use std::rc::Rc;

pub const ERASE_BYTES: usize = 4096;
pub const FLASH_BYTES: usize = ERASE_BYTES * 6;
pub const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0, ERASE_BYTES as u32],
    [
        FlashArenaRange::new((ERASE_BYTES * 2) as u32, (ERASE_BYTES * 4) as u32),
        FlashArenaRange::new((ERASE_BYTES * 4) as u32, FLASH_BYTES as u32),
    ],
);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FlashOperation {
    Read { offset: usize, len: usize },
    Write { offset: usize, bytes: Vec<u8> },
    Erase { from: usize, to: usize },
}
#[derive(Clone)]
pub struct Image {
    bytes: Rc<RefCell<Vec<u8>>>,
    operations: Rc<RefCell<Vec<FlashOperation>>>,
}
impl Image {
    pub fn new() -> Self {
        Self {
            bytes: Rc::new(RefCell::new(vec![0xff; FLASH_BYTES])),
            operations: Rc::new(RefCell::new(Vec::new())),
        }
    }
    pub fn trace(&self) -> Vec<FlashOperation> {
        self.operations.borrow().clone()
    }
    fn record(&self, operation: FlashOperation) {
        let mut operations = self.operations.borrow_mut();
        assert!(operations.len() < 32768, "bounded NOR trace");
        operations.push(operation);
    }
}
#[derive(Debug)]
pub struct FlashError;
impl NorFlashError for FlashError {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}
impl ErrorType for Image {
    type Error = FlashError;
}
impl ReadNorFlash for Image {
    const READ_SIZE: usize = 1;
    async fn read(&mut self, offset: u32, output: &mut [u8]) -> Result<(), FlashError> {
        let offset = offset as usize;
        self.record(FlashOperation::Read {
            offset,
            len: output.len(),
        });
        output.copy_from_slice(&self.bytes.borrow()[offset..offset + output.len()]);
        Ok(())
    }
    fn capacity(&self) -> usize {
        FLASH_BYTES
    }
}
impl NorFlash for Image {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = ERASE_BYTES;
    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), FlashError> {
        let offset = offset as usize;
        assert!(
            offset.is_multiple_of(Self::WRITE_SIZE) && bytes.len().is_multiple_of(Self::WRITE_SIZE)
        );
        self.record(FlashOperation::Write {
            offset,
            bytes: bytes.to_vec(),
        });
        for (stored, written) in self.bytes.borrow_mut()[offset..offset + bytes.len()]
            .iter_mut()
            .zip(bytes)
        {
            assert_eq!(
                *stored & written,
                *written,
                "NOR only programs cleared bits"
            );
            *stored = *written;
        }
        Ok(())
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), FlashError> {
        let from = from as usize;
        let to = to as usize;
        assert!(from.is_multiple_of(Self::ERASE_SIZE) && to.is_multiple_of(Self::ERASE_SIZE));
        self.record(FlashOperation::Erase { from, to });
        self.bytes.borrow_mut()[from..to].fill(0xff);
        Ok(())
    }
}
