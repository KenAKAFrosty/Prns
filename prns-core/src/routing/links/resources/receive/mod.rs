//! The receiver's half of RNS 1.4.0's resource transfer, one file per phase of an incoming transfer's life: the [`gate`] admits or refuses the advertisement, [`rounds`] pumps part requests until the register fills, [`offload`] lends the streamed open's chews to a pool worker, [`conclude`] verifies, proves, and delivers (or fails by name), [`cancel`] handles the sender's mid-flight abort, and the [`watchdog`] enforces every deadline.

pub mod cancel;
pub mod conclude;
pub mod gate;
pub mod offload;
pub mod rounds;
#[cfg(test)]
pub mod tests_support;
pub mod watchdog;
