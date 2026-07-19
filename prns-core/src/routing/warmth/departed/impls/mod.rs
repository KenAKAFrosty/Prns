mod fixed;

pub use fixed::FixedDepartedInterfaceTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapDepartedInterfaceTable;
    }
}
