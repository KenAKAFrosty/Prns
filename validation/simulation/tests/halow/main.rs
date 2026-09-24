#![cfg(feature = "controlled-time")]
#![expect(
    clippy::expect_used,
    reason = "qualification fails at the owning bounded invariant"
)]
#![expect(
    clippy::panic,
    reason = "test-only invariant failures retain complete replay evidence"
)]

mod campaign;
mod fixture;
mod stability;
mod traffic;
mod workloads;

#[cfg(feature = "heap-profile")]
#[global_allocator]
static HEAP: dhat::Alloc = dhat::Alloc;
