mod fixed_incoming_assembly_columns;
pub use fixed_incoming_assembly_columns::FixedIncomingAssemblyColumns;

#[cfg(feature = "alloc")]
mod heap_incoming_assembly_columns;
#[cfg(feature = "alloc")]
pub use heap_incoming_assembly_columns::{
    HeapIncomingAssemblyColumns, DEFAULT_MAX_INCOMING_ASSEMBLIES,
};
