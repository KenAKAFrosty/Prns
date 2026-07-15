cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod file;

        pub use file::{FileStore, FileStoreError};
    }
}
