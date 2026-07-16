mod fixed;

pub use fixed::{FixedBlackholeInsertError, FixedBlackholeTable};

#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapBlackholeTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapBlackholeTable;
    }
}
