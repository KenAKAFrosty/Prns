//! `Prns::run` — the consumer entry point. It performs the platform-neutral assembly the
//! reactor's call sites hand-roll today (build the engine on the recipe's storage, take the
//! transport role, register the starting destinations), then hands the engine to the recipe's
//! [`Bind`] to drive forever. Hopspot, the benchmarks, and every future app reduce to building
//! a [`Recipe`] and awaiting this.

use crate::engine::EngineState;

use super::{Bind, PrnsEvent, Recipe, StartingDestination, Transport};

pub struct Prns;

impl Prns {
    /// Stand up a node from `recipe` and run it until it stops (it does not). Every engine
    /// `Journaled` is forwarded to `on_event`; the app issues commands through the sender it
    /// kept when it built the recipe's [`Bind`].
    pub async fn run<'a, B, D>(recipe: Recipe<B, D>, on_event: impl FnMut(PrnsEvent<'_>)) -> !
    where
        B: Bind,
        D: IntoIterator<Item = StartingDestination<'a>>,
    {
        let Recipe {
            identity,
            transport,
            destinations,
            bind,
        } = recipe;

        let mut engine = EngineState::<B::Storage>::new(identity);
        let primary = engine.held_identity_hashes()[0];
        if let Transport::Node = transport {
            engine
                .set_transport_identity(&primary)
                .expect("the primary identity takes the transport role");
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
                    app_data,
                    proof,
                    ratchet,
                    announce: _,
                } => {
                    engine
                        .register_single_destination(
                            &primary, app_name, aspects, app_data, proof, ratchet,
                        )
                        .expect("recipe single destination is valid");
                }
            }
        }

        bind.drive(engine, on_event).await
    }
}
