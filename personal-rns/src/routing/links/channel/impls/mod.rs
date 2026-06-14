mod fixed_array_channel_columns;
pub use fixed_array_channel_columns::FixedArrayChannelColumns;

#[cfg(feature = "alloc")]
mod heap_channel_columns;
#[cfg(feature = "alloc")]
pub use heap_channel_columns::HeapChannelColumns;

#[cfg(feature = "external-alloc")]
mod fixed_heap_channel_columns;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_channel_columns::FixedHeapChannelColumns;
