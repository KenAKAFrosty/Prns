mod fixed;

pub use fixed::{FixedBlackholeInsertError, FixedBlackholeTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapBlackholeTable;
    }
}
