mod fixed_self_ratchet_columns;
pub use fixed_self_ratchet_columns::FixedSelfRatchetColumns;

#[cfg(feature = "alloc")]
mod heap_self_ratchet_columns;
#[cfg(feature = "alloc")]
pub use heap_self_ratchet_columns::{HeapSelfRatchetColumns, DEFAULT_RETAINED_RATCHETS};
