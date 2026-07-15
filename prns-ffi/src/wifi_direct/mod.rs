cfg_if::cfg_if! {
    if #[cfg(target_os = "android")] {
        pub mod android;
    } else if #[cfg(target_os = "windows")] {
        pub mod windows;
    }
}
