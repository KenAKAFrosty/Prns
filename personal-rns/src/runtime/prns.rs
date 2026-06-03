//! The consumer entry point. An application's `main` constructs its interfaces, then
//! hands [`Prns::run`] a [`Recipe`] — who the node is, what it announces, those
//! interfaces, and the platform [`Host`] — and the container builds the announcing
//! engine, bolts on the [`Runtime`], and drives it forever.
//!
//! Everything below this call (engine, runtime, interface pooling) is internal: the
//! recipe is the whole surface a node operator names.

use core::marker::PhantomData;

use super::{Host, Runtime, RuntimeSnapshot};
use crate::engine::{EngineState, SelfAnnounceConfig};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::RegisteredInterface;
use crate::routing::storage::Storage;

/// Everything [`Prns::run`] needs to stand a node up: the secret key it *is*, what it
/// announces about itself, its already-started interfaces, and the [`Host`] that owns
/// the clock, entropy, and wake. The interface set `I` and host `Ho` were built by the
/// platform's `main`; `Prns`'s storage parameter `S` decides the engine's capacity.
pub struct Recipe<'announce, Ho, I> {
    pub identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    pub self_announce: SelfAnnounceConfig<'announce>,
    pub interfaces: I,
    pub host: Ho,
}

/// The node container, parameterized by the engine-state storage `S` (e.g.
/// [`FixedCapacity`](crate::routing::storage::FixedCapacity)). Carries no state —
/// it exists so `Prns::<S>::run(recipe, observe)` reads as the one entry point.
pub struct Prns<S>(PhantomData<S>);

impl<S: Storage> Prns<S> {
    /// Build the announcing engine from the recipe, pool its interfaces into a
    /// [`Runtime`], and drive it forever — `on_snapshot` sees each cycle's snapshot.
    /// Never returns; an invalid [`SelfAnnounceConfig`] in the recipe is a
    /// construction-time programming error and panics here (the caller's config is
    /// static), as does any other engine-build failure.
    pub async fn run<Ho, I, OnSnapshot>(recipe: Recipe<'_, Ho, I>, on_snapshot: OnSnapshot) -> !
    where
        Ho: Host,
        I: InterfaceSet,
        I::Item: RegisteredInterface,
        OnSnapshot: FnMut(&RuntimeSnapshot),
    {
        let Recipe {
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
