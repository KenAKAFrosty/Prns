use embassy_futures::yield_now;
use embedded_storage::nor_flash::{
    check_erase, check_read, check_write, ErrorType, NorFlash, NorFlashError, NorFlashErrorKind,
    ReadNorFlash,
};
use embedded_storage_async::nor_flash::{
    NorFlash as AsyncNorFlash, ReadNorFlash as AsyncReadNorFlash,
};
use esp_hal::rom::spiflash::{
    esp_rom_spiflash_erase_sector, esp_rom_spiflash_read, esp_rom_spiflash_write,
    ESP_ROM_SPIFLASH_RESULT_OK,
};
#[cfg(target_arch = "xtensa")]
use esp_hal::{
    peripherals::CPU_CTRL,
    system::{is_running, Cpu, CpuControl},
};

const WORD_LEN: usize = 4;
const SECTOR_LEN: usize = 4096;
const BOUNCE_WORDS: usize = 64;
const ATTEMPTS: usize = 3;

#[cfg(target_arch = "xtensa")]
struct OtherCorePark(Option<Cpu>);

#[cfg(target_arch = "xtensa")]
impl OtherCorePark {
    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "the flash operation requires exclusive access while the other CPU is running"
    )]
    fn acquire() -> Self {
        let core = Cpu::other().find(|core| is_running(*core));
        if let Some(core) = core {
            let mut control = CpuControl::new(unsafe { CPU_CTRL::steal() });
            unsafe {
                control.park_core(core);
            }
        }
        Self(core)
    }
}

#[cfg(target_arch = "xtensa")]
impl Drop for OtherCorePark {
    fn drop(&mut self) {
        if let Some(core) = self.0 {
            unpark_core(core);
        }
    }
}

#[cfg(target_arch = "xtensa")]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the flash operation temporarily borrows the CPU control peripheral"
)]
fn unpark_core(core: Cpu) {
    let mut control = CpuControl::new(unsafe { CPU_CTRL::steal() });
    control.unpark_core(core);
}

pub struct EspRomFlash {
    capacity: usize,
}

impl EspRomFlash {
    pub const fn new(capacity: usize) -> Self {
        Self { capacity }
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

impl ErrorType for EspRomFlash {
    type Error = EspRomFlashError;
}

impl ReadNorFlash for EspRomFlash {
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
        self.capacity
    }
}

impl NorFlash for EspRomFlash {
    const WRITE_SIZE: usize = WORD_LEN;
    const ERASE_SIZE: usize = SECTOR_LEN;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        check_erase(self, from, to).map_err(EspRomFlashError::Contract)?;
        for sector in from as usize / SECTOR_LEN..to as usize / SECTOR_LEN {
            let sector = sector as u32;
            if !sector_is_erased(sector)? {
                erase_sector(sector)?;
            }
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

impl AsyncReadNorFlash for EspRomFlash {
    const READ_SIZE: usize = WORD_LEN;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let result = ReadNorFlash::read(self, offset, bytes);
        yield_now().await;
        result
    }

    fn capacity(&self) -> usize {
        ReadNorFlash::capacity(self)
    }
}

impl AsyncNorFlash for EspRomFlash {
    const WRITE_SIZE: usize = WORD_LEN;
    const ERASE_SIZE: usize = SECTOR_LEN;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        check_erase(self, from, to).map_err(EspRomFlashError::Contract)?;
        for sector in from as usize / SECTOR_LEN..to as usize / SECTOR_LEN {
            let sector = sector as u32;
            if !sector_is_erased(sector)? {
                erase_sector(sector)?;
            }
            yield_now().await;
        }
        Ok(())
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let result = NorFlash::write(self, offset, bytes);
        yield_now().await;
        result
    }
}

fn read_words(
    offset: u32,
    words: &mut [u32; BOUNCE_WORDS],
    len: usize,
) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = critical_section::with(|_| {
            // ROM flash reads suspend the shared flash/PSRAM cache just like writes and erases.
            // Core 0 owns Wi-Fi and may otherwise enter the radio driver while its PSRAM-backed
            // allocations are inaccessible, leaving RX permanently wedged after persistence
            // scans. Keep both cores quiescent for the short, 256-byte ROM transaction.
            #[cfg(target_arch = "xtensa")]
            let _park = OtherCorePark::acquire();
            rom_read(offset, words.as_mut_ptr(), len as u32)
        });
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Read(result));
        }
    }
    Ok(())
}

fn write_words(
    offset: u32,
    words: &[u32; BOUNCE_WORDS],
    len: usize,
) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = critical_section::with(|_| {
            #[cfg(target_arch = "xtensa")]
            let _park = OtherCorePark::acquire();
            rom_write(offset, words.as_ptr(), len as u32)
        });
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Write(result));
        }
    }
    Ok(())
}

fn erase_sector(sector: u32) -> Result<(), EspRomFlashError> {
    for attempt in 0..ATTEMPTS {
        let result = critical_section::with(|_| {
            #[cfg(target_arch = "xtensa")]
            let _park = OtherCorePark::acquire();
            rom_erase_sector(sector)
        });
        if result == ESP_ROM_SPIFLASH_RESULT_OK {
            return Ok(());
        }
        if attempt + 1 == ATTEMPTS {
            return Err(EspRomFlashError::Erase(result));
        }
    }
    Ok(())
}

fn sector_is_erased(sector: u32) -> Result<bool, EspRomFlashError> {
    let base = sector * SECTOR_LEN as u32;
    for offset in (0..SECTOR_LEN).step_by(BOUNCE_WORDS * WORD_LEN) {
        let mut bounce = [0u32; BOUNCE_WORDS];
        read_words(base + offset as u32, &mut bounce, BOUNCE_WORDS * WORD_LEN)?;
        if bounce.iter().any(|word| *word != u32::MAX) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg_attr(target_arch = "xtensa", esp_hal::ram)]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the ROM receives an aligned writable word buffer for the exact byte length"
)]
fn rom_read(offset: u32, words: *mut u32, len: u32) -> i32 {
    unsafe { esp_rom_spiflash_read(offset, words, len) }
}

#[cfg_attr(target_arch = "xtensa", esp_hal::ram)]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the ROM receives an aligned readable word buffer for the exact byte length"
)]
fn rom_write(offset: u32, words: *const u32, len: u32) -> i32 {
    unsafe { esp_rom_spiflash_write(offset, words, len) }
}

#[cfg_attr(target_arch = "xtensa", esp_hal::ram)]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the validated sector number is inside the configured flash capacity"
)]
fn rom_erase_sector(sector: u32) -> i32 {
    unsafe { esp_rom_spiflash_erase_sector(sector) }
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
