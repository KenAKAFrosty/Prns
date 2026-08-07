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
        mod flash_journal;

        pub use flash::{
            FlashTimebase, FlashTimebaseError, TIMEBASE_HEADROOM_MILLIS,
            TIMEBASE_RECORD_INTERVAL_MILLIS,
        };
        pub use flash_journal::{
            flash_journal_record_storage_len, FlashArenaRange, FlashJournal, FlashJournalError,
            FlashJournalLayout, FlashJournalRecord, FlashJournalRecordKind,
            FlashJournalRestoreReport, FlashJournalTimebaseState, FlashJournalWarning,
            FLASH_JOURNAL_RECORD_OVERHEAD,
        };
    }
}
