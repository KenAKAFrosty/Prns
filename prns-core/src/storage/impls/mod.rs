mod esp32c6;
mod nrf52840;

pub use esp32c6::Esp32C6;
pub use nrf52840::Nrf52840;

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod esp32s3;

        pub use esp32s3::Esp32S3;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod growable_heap;

        pub use growable_heap::GrowableHeap;
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(test, feature = "test-support"))] {
        mod test_fixed_storage;

        pub use test_fixed_storage::TestFixedStorage;
    }
}
