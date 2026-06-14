//! `Prns::run` — the consumer entry point. It performs the platform-neutral assembly the
//! reactor's call sites hand-roll today (start from an empty engine on the recipe's storage,
//! take the transport role if asked, hold each destination's key and register it), then hands
//! the engine to the recipe's [`Bind`] to drive forever. Hopspot, the benchmarks, and every
//! future app reduce to building a [`Recipe`] and awaiting this.

use crate::engine::EngineState;

use super::{Bind, PrnsEvent, Recipe, StartingDestination};

pub struct Prns;

impl Prns {
    /// Stand up a node from `recipe` and run it until it stops (the reactor loops indefinitely,
    /// so in practice this never returns). Every engine event is mapped to a [`PrnsEvent`] and
    /// handed to `on_event`; the app issues commands through the sender it kept when it built the
    /// recipe's [`Bind`].
    #[allow(clippy::expect_used)]
    pub async fn run<'a, B, D>(recipe: Recipe<B, D>, on_event: impl FnMut(PrnsEvent<'_>))
    where
        B: Bind,
        D: IntoIterator<Item = StartingDestination<'a>>,
    {
        let Recipe {
            transport,
            destinations,
            bind,
        } = recipe;

        // No self: the engine starts empty. Each destination brings whatever identity it needs.
        let mut engine = EngineState::<B::Storage>::default();
        if let Some(id) = transport {
            engine.set_transport_id(id);
        }

        for destination in destinations {
            match destination {
                StartingDestination::Plain { app_name, aspects } => {
                    engine
                        .register_plain_destination(app_name, aspects)
                        .expect("recipe plain destination is valid");
                }
                StartingDestination::Single {
                    app_name,
                    aspects,
                    identity,
                    app_data,
                    proof,
                    ratchet,
                } => {
                    let held = engine
                        .hold_identity(identity)
                        .expect("recipe identity fits the store");
                    engine
                        .register_single_destination(
                            &held, app_name, aspects, app_data, proof, ratchet,
                        )
                        .expect("recipe single destination is valid");
                }
            }
        }

        bind.drive(engine, on_event).await
    }
}
