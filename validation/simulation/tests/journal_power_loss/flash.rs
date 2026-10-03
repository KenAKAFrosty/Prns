use embedded_storage_async::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};

pub const PAGE: usize = 256;
pub const CAPACITY: usize = PAGE * 6;
const TRACE_CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image(pub [u8; CAPACITY]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Read { offset: usize, len: usize },
    Write { offset: usize, len: usize },
    Erase { offset: usize, len: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub operation: usize,
    pub completed_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    PowerLost,
    Misaligned,
    OutOfBounds,
    RequiresErase,
}

impl NorFlashError for Error {
    fn kind(&self) -> NorFlashErrorKind {
        match self {
            Self::Misaligned => NorFlashErrorKind::NotAligned,
            Self::OutOfBounds => NorFlashErrorKind::OutOfBounds,
            Self::PowerLost | Self::RequiresErase => NorFlashErrorKind::Other,
        }
    }
}

enum Power {
    On,
    CutAt(Cut),
    Off,
}

pub struct Flash {
    image: Image,
    power: Power,
    trace: Vec<Operation>,
}

impl Flash {
    pub fn boot(image: Image) -> Self {
        Self {
            image,
            power: Power::On,
            trace: Vec::new(),
        }
    }

    pub fn arm(&mut self, cut: Cut) {
        assert!(self.trace.is_empty(), "arm before the first boot operation");
        self.power = Power::CutAt(cut);
    }

    pub fn into_image(self) -> Image {
        self.image
    }

    pub fn trace(&self) -> &[Operation] {
        &self.trace
    }

    fn range(&self, offset: usize, len: usize, alignment: usize) -> Result<(), Error> {
        if matches!(self.power, Power::Off) {
            return Err(Error::PowerLost);
        }
        if !offset.is_multiple_of(alignment) || !len.is_multiple_of(alignment) {
            return Err(Error::Misaligned);
        }
        if offset.checked_add(len).is_none_or(|end| end > CAPACITY) {
            return Err(Error::OutOfBounds);
        }
        Ok(())
    }

    fn begin(&mut self, operation: Operation, len: usize) -> usize {
        assert!(self.trace.len() < TRACE_CAPACITY, "bounded flash trace");
        let ordinal = self.trace.len();
        self.trace.push(operation);
        if let Power::CutAt(cut) = self.power {
            if ordinal == cut.operation {
                assert!(cut.completed_bytes <= len, "invalid cut prefix");
                self.power = Power::Off;
                return cut.completed_bytes;
            }
        }
        len
    }

    fn finish(&self) -> Result<(), Error> {
        match self.power {
            Power::Off => Err(Error::PowerLost),
            Power::On | Power::CutAt(_) => Ok(()),
        }
    }
}

impl ErrorType for Flash {
    type Error = Error;
}

impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Error> {
        let offset = offset as usize;
        self.range(offset, bytes.len(), Self::READ_SIZE)?;
        let len = self.begin(
            Operation::Read {
                offset,
                len: bytes.len(),
            },
            bytes.len(),
        );
        bytes[..len].copy_from_slice(&self.image.0[offset..offset + len]);
        self.finish()
    }

    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = PAGE;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        let offset = offset as usize;
        self.range(offset, bytes.len(), Self::WRITE_SIZE)?;
        if self.image.0[offset..offset + bytes.len()]
            .iter()
            .zip(bytes)
            .any(|(old, new)| old & new != *new)
        {
            return Err(Error::RequiresErase);
        }
        let len = self.begin(
            Operation::Write {
                offset,
                len: bytes.len(),
            },
            bytes.len(),
        );
        self.image.0[offset..offset + len].copy_from_slice(&bytes[..len]);
        self.finish()
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        let offset = from as usize;
        let len = (to as usize)
            .checked_sub(offset)
            .ok_or(Error::OutOfBounds)?;
        self.range(offset, len, Self::ERASE_SIZE)?;
        let completed = self.begin(Operation::Erase { offset, len }, len);
        self.image.0[offset..offset + completed].fill(0xff);
        self.finish()
    }
}
