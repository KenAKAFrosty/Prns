cfg_if::cfg_if! {
    if #[cfg(any(target_os = "macos", target_os = "ios"))] {
        pub mod macos;
    } else if #[cfg(target_os = "android")] {
        pub mod android;
    } else if #[cfg(target_os = "windows")] {
        pub mod windows;
    }
}
