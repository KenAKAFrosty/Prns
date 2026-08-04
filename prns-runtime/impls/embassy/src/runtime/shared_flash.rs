use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_storage_async::nor_flash::{ErrorType, NorFlash, ReadNorFlash};

/// A copyable view of one asynchronously serialized NOR flash device.
///
/// Each operation holds the device lock through completion. Higher-level stores may interleave
/// operations safely when their flash regions are disjoint.
pub struct SharedNorFlash<'a, M, F>
where
    M: RawMutex,
{
    flash: &'a Mutex<M, F>,
    capacity: usize,
}

impl<'a, M, F> SharedNorFlash<'a, M, F>
where
    M: RawMutex,
{
    #[must_use]
    pub const fn new(flash: &'a Mutex<M, F>, capacity: usize) -> Self {
        Self { flash, capacity }
    }
}

impl<M, F> Clone for SharedNorFlash<'_, M, F>
where
    M: RawMutex,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M, F> Copy for SharedNorFlash<'_, M, F> where M: RawMutex {}

impl<M, F> ErrorType for SharedNorFlash<'_, M, F>
where
    M: RawMutex,
    F: ErrorType,
{
    type Error = F::Error;
}

impl<M, F> ReadNorFlash for SharedNorFlash<'_, M, F>
where
    M: RawMutex,
    F: ReadNorFlash,
{
    const READ_SIZE: usize = F::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.flash.lock().await.read(offset, bytes).await
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<M, F> NorFlash for SharedNorFlash<'_, M, F>
where
    M: RawMutex,
    F: NorFlash,
{
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = F::ERASE_SIZE;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.flash.lock().await.erase(from, to).await
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.flash.lock().await.write(offset, bytes).await
    }
}
