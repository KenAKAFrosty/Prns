pub mod core;
pub use self::core::*;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod impls;

        pub use impls::*;
    }
}
