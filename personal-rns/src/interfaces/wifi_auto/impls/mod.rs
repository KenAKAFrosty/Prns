#[cfg(feature = "wifi-lan-auto")]
pub mod tokio;

#[cfg(feature = "embassy-wifi")]
pub mod embassy;
