//! The consumer entry point. An application's `main` constructs its host and
//! interfaces, then hands [`Prns::run`] a [`Recipe`] — who the node is, what it
//! announces, the engine's storage capacity, those interfaces, and the platform
//! [`Host`] — and the container builds the announcing engine, bolts on the
//! [`Runtime`], and drives it forever.
//!
//! Everything below this call (engine, runtime, interface pooling) is internal: the
//! recipe is the whole surface a node operator names.

use super::{Host, Runtime, RuntimeSnapshot};
use crate::engine::{EngineState, SelfAnnounceConfig};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::RegisteredInterface;
use crate::routing::storage::EngineStorage;

/// Everything [`Prns::run`] needs to stand a node up: the engine-storage capacity it
/// runs on, the secret key it *is*, what it announces about itself, its already-started
/// interfaces, and the [`Host`] that owns the clock, entropy, and wake.
///
/// `engine_storage` is a zero-sized capacity marker (e.g. [`FixedCapacity::DEFAULT`])
/// whose *type* `S` decides the routing table's sizing — carrying it as a value is what
/// lets `Prns::run` infer `S`, so the caller never writes a turbofish. The interface set
/// `I` and host `Ho` are built by the platform's `main`.
///
/// [`FixedCapacity::DEFAULT`]: crate::routing::storage::FixedCapacity::DEFAULT
pub struct Recipe<'announce, S, Ho, I> {
    pub engine_storage: S,
    pub identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    pub self_announce: SelfAnnounceConfig<'announce>,
    pub interfaces: I,
    pub host: Ho,
}

/// The node container. Carries no state — it exists so `Prns::run(recipe, on_snapshot)`
/// reads as the one entry point. The engine-storage type `S` is inferred from the
/// recipe's `engine_storage` value, so there is no turbofish here.
pub struct Prns;

impl Prns {
    /// Build the announcing engine from the recipe, pool its interfaces into a
    /// [`Runtime`], and drive it forever — `on_snapshot` sees each cycle's snapshot.
    /// Never returns; an invalid [`SelfAnnounceConfig`] in the recipe is a
    /// construction-time programming error and panics here (the caller's config is
    /// static), as does any other engine-build failure.
    ///
    /// The whole node is wired up inline in `main` and handed over as one recipe — the
    /// host and the storage capacity are both just built on the fly:
    ///
    /// ```ignore
    /// let host = LinuxSync::new();                    // the platform host, on the fly
    /// let mut interfaces = GrowableInterfaceSet::new();
    /// let _ = interfaces.push(host.attach(my_interface, MAX_BUFFERED_PACKETS));
    ///
    /// block_on(Prns::run(
    ///     Recipe {
    ///         engine_storage: FixedCapacity::DEFAULT, // capacity recipe, as a value (no turbofish)
    ///         identity_secret_key,
    ///         self_announce,
    ///         interfaces,
    ///         host,
    ///     },
    ///     |snapshot| { /* observe each cycle */ },
    /// ));
    /// ```
    pub async fn run<S, Ho, I, OnSnapshot>(
        recipe: Recipe<'_, S, Ho, I>,
        on_snapshot: OnSnapshot,
    ) -> !
    where
        S: EngineStorage,
        Ho: Host,
        I: InterfaceSet,
        I::Item: RegisteredInterface,
        OnSnapshot: FnMut(&RuntimeSnapshot),
    {
        let Recipe {
            // A zero-sized capacity marker: its *type* `S` is what the engine builds
            // its columns from (via `S`'s `Default` bounds); the value carries nothing.
            engine_storage: _,
            identity_secret_key,
            self_announce,
            interfaces,
            host,
        } = recipe;

        let engine = EngineState::<S>::announcing(&identity_secret_key, self_announce)
            .expect("recipe self-announce config is valid");
        // The engine copied the keys in; wipe ours promptly.
        drop(identity_secret_key);

        let runtime = Runtime::new(engine, interfaces, host);
        runtime.run(on_snapshot).await
    }
}
