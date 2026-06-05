use super::{Host, PrnsEvent, Runtime};
use crate::engine::self_announce::AnnounceConfig;
use crate::engine::EngineState;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::RegisteredInterface;
use crate::routing::storage::EngineStorage;

/// One destination this node serves from the moment it starts. The runtime
/// control surface will let apps register more later; these are the ones the
/// Recipe stands up before the first packet is ingested.
pub enum DestinationConfig<'a> {
    Plain {
        app_name: &'a str,
        aspects: &'a [&'a str],
    },
    Single {
        app_name: &'a str,
        aspects: &'a [&'a str],
        identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        announce: Option<AnnounceConfig<'a>>,
    },
}

/// Everything [`Prns::run`] needs to stand a node up: the engine-storage capacity it
/// runs on, the destinations it starts serving (each Single carrying the identity it
/// answers as, and optionally what it announces), its already-started interfaces, and
/// the [`Host`] that owns the clock, entropy, and wake.
///
/// `engine_storage` is a zero-sized capacity marker (a `FixedInline<…>` the host
/// spells with its own sizing) whose *type* `S` decides the routing table's sizing —
/// carrying it as a value is what lets `Prns::run` infer `S`, so the caller never writes
/// a turbofish. The interface set `I` and host `Ho` are built by the platform's `main`.
pub struct Recipe<S, Ho, I, D> {
    pub engine_storage: S,
    pub starting_destinations: D,
    pub interfaces: I,
    pub host: Ho,
}

pub struct Prns;

impl Prns {
    #[allow(clippy::expect_used)]
    pub async fn run<'a, S, Ho, I, D, OnEvent>(recipe: Recipe<S, Ho, I, D>, on_event: OnEvent) -> !
    where
        S: EngineStorage,
        Ho: Host,
        I: InterfaceSet,
        I::Item: RegisteredInterface,
        D: IntoIterator<Item = DestinationConfig<'a>>,
        OnEvent: FnMut(PrnsEvent<'_>),
    {
        let Recipe {
            engine_storage: _,
            starting_destinations,
            interfaces,
            host,
        } = recipe;

        let mut engine = EngineState::<S>::default();
        for destination in starting_destinations {
            match destination {
                DestinationConfig::Plain { app_name, aspects } => {
                    engine
                        .register_plain_destination(app_name, aspects)
                        .expect("recipe plain destination config is valid");
                }
                DestinationConfig::Single {
                    app_name,
                    aspects,
                    identity_secret_key,
                    announce,
                } => {
                    let identity = engine
                        .hold_identity(identity_secret_key)
                        .expect("recipe identities fit the store");
                    let registered = engine
                        .register_single_destination(&identity, app_name, aspects)
                        .expect("recipe single destination config is valid");
                    if let Some(announce) = announce {
                        engine
                            .schedule_announce(&registered, announce)
                            .expect("recipe announce config is valid");
                    }
                }
            }
        }

        let runtime = Runtime::new(engine, interfaces, host);
        runtime.run(on_event).await
    }
}
