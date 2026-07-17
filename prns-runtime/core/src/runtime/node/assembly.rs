use core::marker::PhantomData;

use crate::engine::EngineState;
use crate::storage::StorageLayout;

use super::super::request_router::RouteSet;
use super::super::PrnsEvent;
use super::recipe::{PreConfiguredDestination, PrnsRecipe};

pub(crate) struct AssembledNode<St, R, F, S>
where
    S: StorageLayout,
{
    pub(crate) engine: EngineState<S>,
    pub(crate) state: St,
    pub(crate) on_event: F,
    pub(crate) routes: PhantomData<R>,
}

#[allow(clippy::expect_used)]
pub(crate) fn assemble_node<'a, D, St, R, F, I, S>(
    recipe: PrnsRecipe<D, St, R, F, I, S>,
) -> (AssembledNode<St, R, F, S>, I)
where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let PrnsRecipe {
        transport_identity,
        pre_configured_destinations,
        app_state,
        storage: _,
        routes: _,
        interfaces,
        on_event,
    } = recipe;

    let mut engine = EngineState::<S>::default();
    for destination in pre_configured_destinations {
        match destination {
            PreConfiguredDestination::Plain { app_name, aspects } => {
                engine
                    .register_plain_destination(app_name, aspects)
                    .expect("recipe plain destination is valid");
            }
            PreConfiguredDestination::Single {
                app_name,
                aspects,
                identity,
                announce_app_data: app_data,
                proof,
                link_requests,
                ratchet,
                resource_strategy,
            } => {
                let held = engine
                    .hold_identity(identity)
                    .expect("recipe identity fits the store");
                let destination = engine
                    .register_single_destination(
                        &held,
                        app_name,
                        aspects,
                        app_data,
                        proof,
                        link_requests,
                        ratchet,
                    )
                    .expect("recipe single destination is valid");
                engine.set_default_resource_strategy(&destination, resource_strategy);
                for (path, policy) in R::REGISTRATIONS {
                    engine
                        .register_request_handler(&destination, path, policy.engine_policy())
                        .expect("recipe request handler fits the store");
                    for seed in policy.seed_list() {
                        engine
                            .allow_requester(&destination, path, *seed)
                            .expect("recipe seed identity admits to its own fresh handler");
                    }
                }
            }
        }
    }

    if let Some(secret) = transport_identity {
        let identity = engine
            .hold_identity(secret)
            .expect("the transport identity fits the held-identity store");
        engine
            .set_transport_identity(&identity)
            .expect("the transport identity was just held");
    }

    (
        AssembledNode {
            engine,
            state: app_state,
            on_event,
            routes: PhantomData,
        },
        interfaces,
    )
}
