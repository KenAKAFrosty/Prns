#[cfg(any(feature = "embassy-host", test))]
pub(super) mod fixed;

#[cfg(feature = "tokio-host")]
pub(super) mod heap;
