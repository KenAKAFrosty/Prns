use super::{Host, PrnsEvent, Runtime};
use crate::engine::{EngineState, SelfAnnounceConfig};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::RegisteredInterface;
use crate::routing::storage::EngineStorage;

/// Everything [`Prns::run`] needs to stand a node up: the engine-storage capacity it
/// runs on, the secret key it *is*, what it announces about itself, its already-started
/// interfaces, and the [`Host`] that owns the clock, entropy, and wake.
///
/// `engine_storage` is a zero-sized capacity marker (a `FixedInline<…>` the host
/// spells with its own sizing) whose *type* `S` decides the routing table's sizing —
/// carrying it as a value is what lets `Prns::run` infer `S`, so the caller never writes
/// a turbofish. The interface set `I` and host `Ho` are built by the platform's `main`.
pub struct Recipe<'announce, S, Ho, I> {
    pub engine_storage: S,
    pub identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    pub self_announce: SelfAnnounceConfig<'announce>,
    pub interfaces: I,
    pub host: Ho,
}

pub struct Prns;

impl Prns {
    #[allow(clippy::expect_used)]
    pub async fn run<S, Ho, I, OnEvent>(recipe: Recipe<'_, S, Ho, I>, on_event: OnEvent) -> !
    where
        S: EngineStorage,
        Ho: Host,
        I: InterfaceSet,
        I::Item: RegisteredInterface,
        OnEvent: FnMut(PrnsEvent<'_>),
    {
        let Recipe {
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
        runtime.run(on_event).await
    }
}
