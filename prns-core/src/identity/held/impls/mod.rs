mod fixed_held_identity_columns;
pub use fixed_held_identity_columns::FixedHeldIdentityColumns;

#[cfg(feature = "alloc")]
mod heap_held_identity_columns;
#[cfg(feature = "alloc")]
pub use heap_held_identity_columns::HeapHeldIdentityColumns;
