#![expect(
    clippy::expect_used,
    reason = "explicit bounded qualification fixtures"
)]
#![expect(
    clippy::panic,
    reason = "qualification records failure evidence before failing"
)]
mod campaign;
mod fixture;
mod operation;
mod profile;
mod stability;
mod storage;
mod tests;
mod traffic;
mod workloads;
