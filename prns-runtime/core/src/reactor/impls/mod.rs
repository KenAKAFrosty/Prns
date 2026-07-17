cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        pub mod compression;
        mod tokio_grant_lane;
        pub mod tokio_reactor;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "embassy-host")] {
        pub mod embassy_reactor;
    }
}
