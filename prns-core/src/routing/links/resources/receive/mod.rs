//! The receiver's half of RNS 1.4.2's resource transfer, one file per phase of an incoming transfer's life: the [`gate`] admits or refuses the advertisement, [`rounds`] pumps part requests until the register fills, [`offload`] lends the streamed open's chews to a pool worker, [`conclude`] verifies, proves, and delivers (or fails by name), [`cancel`] handles the sender's mid-flight abort, and the [`watchdog`] enforces every deadline.

pub mod cancel;
pub mod conclude;
pub mod gate;
#[cfg(test)]
mod metadata_response_limits;
pub mod offload;
pub mod part_hash;
pub mod rounds;
#[cfg(test)]
mod split_admission_tests;
mod split_delivery;
#[cfg(test)]
mod split_ownership_tests;
#[cfg(test)]
mod split_response_failures_tests;
#[cfg(test)]
pub mod tests_support;
pub mod watchdog;
#[cfg(test)]
mod whole_response_limits;
