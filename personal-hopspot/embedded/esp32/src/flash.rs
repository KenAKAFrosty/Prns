use embedded_storage::nor_flash::{
    check_erase, check_read, check_write, ErrorType, NorFlash, NorFlashError, NorFlashErrorKind,
    ReadNorFlash,
};
use esp_hal::rom::spiflash::{
    esp_rom_spiflash_erase_sector, esp_rom_spiflash_read, esp_rom_spiflash_write,
    ESP_ROM_SPIFLASH_RESULT_OK,
};

const WORD_LEN: usize = 4;
const SECTOR_LEN: usize = 4096;
const BOUNCE_WORDS: usize = 64;
const ATTEMPTS: usize = 3;

pub struct EspRomFlash<const CAPACITY: usize>;

impl<const CAPACITY: usize> EspRomFlash<CAPACITY> {
    pub const fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspRomFlashError {
    Contract(NorFlashErrorKind),
    Read(i32),
    Write(i32),
    Erase(i32),
}

impl NorFlashError for EspRomFlashError {
    fn kind(&self) -> NorFlashErrorKind {
        match self {
            Self::Contract(kind) => *kind,
            Self::Read(_) | Self::Write(_) | Self::Erase(_) => NorFlashErrorKind::Other,
        }
    }
}

impl<const CAPACITY: usize> ErrorType for EspRomFlash<CAPACITY> {
    type Error = EspRomFlashError;
}

impl<const CAPACITY: usize> ReadNorFlash for EspRomFlash<CAPACITY> {
    const READ_SIZE: usize = WORD_LEN;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        check_read(self, offset, bytes.len()).map_err(EspRomFlashError::Contract)?;
        let mut at = offset;
        for chunk in bytes.chunks_mut(BOUNCE_WORDS * WORD_LEN) {
            let mut bounce = [0u32; BOUNCE_WORDS];
            read_words(at, &mut bounce, chunk.len())?;
            for (destination, word) in chunk.chunks_exact_mut(WORD_LEN).zip(bounce) {
                destination.copy_from_slice(&word.to_le_bytes());
            }
            at += chunk.len() as u32;
        }
        Ok(())
    }

    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl<const CAPACITY: usize> NorFlash for EspRomFlash<CAPACITY> {
    const WRITE_SIZE: usize = WORD_LEN;
    const ERASE_SIZE: usize = SECTOR_LEN;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        check_erase(self, from, to).map_err(EspRomFlashError::Contract)?;
        for sector in from as usize / SECTOR_LEN..to as usize / SECTOR_LEN {
            erase_sector(sector as u32)?;
        }
        Ok(())
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        check_write(self, offset, bytes.len()).map_err(EspRomFlashError::Contract)?;
        let mut at = offset;
        for chunk in bytes.chunks(BOUNCE_WORDS * WORD_LEN) {
            let mut bounce = [0u32; BOUNCE_WORDS];
            for (word, source) in bounce.iter_mut().zip(chunk.chunks_exact(WORD_LEN)) {
                *word = u32::from_le_bytes([source[0], source[1], source[2], source[3]]);
            }
            write_words(at, &bounce, chunk.len())?;
            at += chunk.len() as u32;
        }
        Ok(())
    }
}

#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the ROM receives an aligned writable word buffer for the exact byte length"
)]
fn read_words(
    offset: u32,
    words: &mut [u32; BOUNCE_WORDS],
    len: usize,
) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = unsafe { esp_rom_spiflash_read(offset, words.as_mut_ptr(), len as u32) };
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Read(result));
        }
    }
    Ok(())
}

#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the ROM receives an aligned readable word buffer for the exact byte length"
)]
fn write_words(
    offset: u32,
    words: &[u32; BOUNCE_WORDS],
    len: usize,
) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = unsafe { esp_rom_spiflash_write(offset, words.as_ptr(), len as u32) };
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Write(result));
        }
    }
    Ok(())
}

#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the validated sector number is inside the configured flash capacity"
)]
fn erase_sector(sector: u32) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = unsafe { esp_rom_spiflash_erase_sector(sector) };
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Erase(result));
        }
    }
    Ok(())
}

impl core::fmt::Display for EspRomFlashError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Contract(kind) => kind.fmt(formatter),
            Self::Read(code) => write!(formatter, "flash read failed with {code}"),
            Self::Write(code) => write!(formatter, "flash write failed with {code}"),
            Self::Erase(code) => write!(formatter, "flash erase failed with {code}"),
        }
    }
}
