//! In-process loopback interfaces — three variants by environment.
//!
//! Same paired-halves semantics across all three: write on one half,
//! read on the other. Each variant trades off against a different
//! constraint axis:
//!
//! - [`alloc`]: heap-backed `VecDeque` queue for single-thread use.
//!   The default reach-for under the `alloc` feature.
//! - [`threaded`]: `Arc<Mutex<…>>`-backed queue for cross-thread use
//!   (`std` only). Same observable behaviour as `alloc`, just `Send`.
//! - [`no_alloc`]: caller-supplied fixed-cap queue stored in a
//!   `RefCell`; pure no_std/no_alloc. The embedded variant.
//!
//! Public types are re-exported flat at [`crate::interfaces`] so
//! consumers don't need to know which submodule owns them.

#[cfg(feature = "alloc")]
pub mod alloc;
#[cfg(feature = "alloc")]
pub use alloc::{LoopbackError, LoopbackInterface};

#[cfg(feature = "std")]
pub mod threaded;
#[cfg(feature = "std")]
pub use threaded::ThreadedLoopback;

pub mod no_alloc;
pub use no_alloc::{NoAllocLoopback, NoAllocLoopbackError, NoAllocLoopbackQueue};
