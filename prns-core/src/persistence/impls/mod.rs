#[cfg(feature = "std")]
mod file;
#[cfg(feature = "std")]
pub use file::{FileStore, FileStoreError};
