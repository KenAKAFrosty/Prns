cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod file;
        pub mod reticulum_directory;

        pub use file::{FileStore, FileStoreError};
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "flash")] {
        mod flash;

        pub use flash::{
            FlashTimebase, FlashTimebaseError, TIMEBASE_HEADROOM_MILLIS,
            TIMEBASE_RECORD_INTERVAL_MILLIS,
        };
    }
}
