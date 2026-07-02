#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod macos;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "windows")]
pub mod windows;
