use allocator_api2::alloc::{AllocError, Allocator};
use core::alloc::Layout;
use core::ptr::NonNull;

use personal_rns::storage::Esp32S3;

/// The engine's storage recipe: hot/synchronized columns stay inline in SRAM, the cold
/// per-destination bulk (routes, announces, history, app-data, resource buffers) is boxed
/// into PSRAM through `PsramAlloc`, each keeping its small index/metadata in SRAM.
pub type EngineStorageType = Esp32S3<PsramAlloc>;

/// A `Default`-able allocator that places allocations in PSRAM. esp-alloc's own
/// `ExternalMemory` targets the same region but is not `Default`, which the column recipes
/// need to build themselves from `StorageLayout`; this thin ZST forwards to it.
#[derive(Default, Clone, Copy)]
pub struct PsramAlloc;

unsafe impl Allocator for PsramAlloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        esp_alloc::ExternalMemory.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { esp_alloc::ExternalMemory.deallocate(ptr, layout) }
    }
}
