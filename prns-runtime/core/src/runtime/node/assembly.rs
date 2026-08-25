use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::engine::EngineState;
use crate::engine::RatchetPolicy;
use crate::identity::held::HoldIdentityError;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::RequestHandlerError;
use crate::routing::upstream_app_destinations::RegisterDestinationError;
use crate::routing::{LinkRequestPolicy, ProofStrategy};
use crate::storage::StorageLayout;
use crate::storage::TablePushError;
use crate::units::ByteLimit;
use crate::wire::DestinationHash;

use super::super::request_endpoints::RequestEndpointSet;
use super::super::PrnsEvent;
use super::recipe::{PreConfiguredDestination, PrnsNodeRecipe, ServeMyRequestEndpoints};

pub struct AssembledNode<St, R, F, S>
where
    S: StorageLayout,
{
    pub engine: EngineState<S>,
    pub state: St,
    pub on_event: F,
    pub request_endpoints: PhantomData<R>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurePreconfiguredDestinationError {
    HoldIdentity(HoldIdentityError),
    Register(RegisterDestinationError),
    RegisterRequestHandler(TablePushError),
    SeedRequester(RequestHandlerError),
    ServesEmptyEndpointSet,
}

struct SingleDestinationConfiguration<'a> {
    app_name: &'a str,
    aspects: &'a [&'a str],
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    app_data: &'a [u8],
    proof: ProofStrategy,
    link_requests: LinkRequestPolicy,
    ratchet: RatchetPolicy,
    resource_strategy: ResourceStrategy,
    maximum_request_bytes: ByteLimit,
}

pub fn configure_preconfigured_destination<'a, St, R, S>(
    engine: &mut EngineState<S>,
    destination: PreConfiguredDestination<'a>,
) -> Result<DestinationHash, ConfigurePreconfiguredDestinationError>
where
    R: RequestEndpointSet<St>,
    S: StorageLayout,
{
    match destination {
        PreConfiguredDestination::Plain { app_name, aspects } => engine
            .register_plain_destination(app_name, aspects)
            .map_err(ConfigurePreconfiguredDestinationError::Register),
        PreConfiguredDestination::Group {
            app_name,
            aspects,
            identity,
            shared_key,
        } => engine
            .register_group_destination(&identity, app_name, aspects, shared_key)
            .map_err(ConfigurePreconfiguredDestinationError::Register),
        PreConfiguredDestination::Single {
            app_name,
            aspects,
            identity,
            announce_app_data,
            proof,
            link_requests,
            ratchet,
            resource_strategy,
            maximum_request_bytes,
            request_endpoints,
        } => configure_single_destination::<St, R, S>(
            engine,
            SingleDestinationConfiguration {
                app_name,
                aspects,
                identity,
                app_data: announce_app_data,
                proof,
                link_requests,
                ratchet,
                resource_strategy,
                maximum_request_bytes,
            },
            request_endpoints,
        ),
    }
}

fn configure_single_destination<St, R, S>(
    engine: &mut EngineState<S>,
    configuration: SingleDestinationConfiguration<'_>,
    request_endpoints: ServeMyRequestEndpoints,
) -> Result<DestinationHash, ConfigurePreconfiguredDestinationError>
where
    R: RequestEndpointSet<St>,
    S: StorageLayout,
{
    let SingleDestinationConfiguration {
        app_name,
        aspects,
        identity,
        app_data,
        proof,
        link_requests,
        ratchet,
        resource_strategy,
        maximum_request_bytes,
    } = configuration;
    let held = engine
        .hold_identity(identity)
        .map_err(ConfigurePreconfiguredDestinationError::HoldIdentity)?;
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
        .map_err(ConfigurePreconfiguredDestinationError::Register)?;
    engine.set_default_resource_strategy(&destination, resource_strategy);
    engine.set_maximum_request_bytes(&destination, maximum_request_bytes);
    if matches!(request_endpoints, ServeMyRequestEndpoints::Yes) {
        register_request_routes_for::<St, R, S>(engine, destination)?;
    }
    Ok(destination)
}

fn register_request_routes_for<St, R, S>(
    engine: &mut EngineState<S>,
    destination: DestinationHash,
) -> Result<(), ConfigurePreconfiguredDestinationError>
where
    R: RequestEndpointSet<St>,
    S: StorageLayout,
{
    for (path, policy) in R::REGISTRATIONS {
        engine
            .register_request_handler(&destination, path, policy.engine_policy())
            .map_err(ConfigurePreconfiguredDestinationError::RegisterRequestHandler)?;
        for seed in policy.seed_list() {
            engine
                .allow_requester(&destination, path, *seed)
                .map_err(ConfigurePreconfiguredDestinationError::SeedRequester)?;
        }
    }
    Ok(())
}

#[allow(clippy::expect_used)]
pub fn assemble_node<'a, D, St, R, F, I, S, P>(
    recipe: PrnsNodeRecipe<D, St, R, F, I, S, P>,
) -> (AssembledNode<St, R, F, S>, I, P)
where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let PrnsNodeRecipe {
        transport_identity,
        pre_configured_destinations,
        app_state,
        storage: _,
        request_endpoints: _,
        interfaces,
        persistence,
        on_event,
    } = recipe;

    let mut node = AssembledNode {
        engine: EngineState::<S>::default(),
        state: app_state,
        on_event,
        request_endpoints: PhantomData,
    };
    configure_assembled_node(&mut node, pre_configured_destinations, transport_identity);
    (node, interfaces, persistence)
}

#[expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "every AssembledNode field is initialized before the slot is exposed"
)]
pub fn assemble_node_in_place<'a, 'slot, D, St, R, F, I, S, P>(
    slot: &'slot mut MaybeUninit<AssembledNode<St, R, F, S>>,
    recipe: PrnsNodeRecipe<D, St, R, F, I, S, P>,
) -> (&'slot mut AssembledNode<St, R, F, S>, I, P)
where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let PrnsNodeRecipe {
        transport_identity,
        pre_configured_destinations,
        app_state,
        storage: _,
        request_endpoints: _,
        interfaces,
        persistence,
        on_event,
    } = recipe;
    let node = slot.as_mut_ptr();
    unsafe {
        let engine =
            &mut *core::ptr::addr_of_mut!((*node).engine).cast::<MaybeUninit<EngineState<S>>>();
        EngineState::init_in_place(engine);
        core::ptr::addr_of_mut!((*node).state).write(app_state);
        core::ptr::addr_of_mut!((*node).on_event).write(on_event);
        core::ptr::addr_of_mut!((*node).request_endpoints).write(PhantomData);
    }
    let node = unsafe { slot.assume_init_mut() };
    configure_assembled_node(node, pre_configured_destinations, transport_identity);
    (node, interfaces, persistence)
}

#[allow(clippy::expect_used)]
fn configure_assembled_node<'a, D, St, R, F, S>(
    node: &mut AssembledNode<St, R, F, S>,
    pre_configured_destinations: D,
    transport_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
) where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let mut any_destination_declared = false;
    let mut any_destination_serves = false;
    for destination in pre_configured_destinations {
        any_destination_declared = true;
        any_destination_serves |= matches!(
            destination,
            PreConfiguredDestination::Single {
                request_endpoints: ServeMyRequestEndpoints::Yes,
                ..
            }
        );
        configure_preconfigured_destination::<St, R, S>(&mut node.engine, destination)
            .expect("recipe destination is valid and fits the store");
    }
    assert!(
        R::REGISTRATIONS.is_empty() || any_destination_serves || !any_destination_declared,
        "the recipe declares request endpoints but no destination serves them; set request_endpoints: ServeMyRequestEndpoints::Yes on a destination"
    );

    if let Some(secret) = transport_identity {
        let identity = node
            .engine
            .hold_identity(secret)
            .expect("the transport identity fits the held-identity store");
        node.engine
            .set_transport_identity(&identity)
            .expect("the transport identity was just held");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityHash;
    use crate::routing::request_handlers::RequestPathHash;
    use crate::runtime::request_endpoints::{Decline, RequestContext, RequestEndpointPolicy};
    use crate::runtime::{ManuallyAttached, NoPersistence};
    use crate::storage::TestFixedStorage;

    type Storage = TestFixedStorage<4, 4, 128, 4, 4, 4, 2, 2, 2, 2, 2, 2>;

    struct Routes;

    impl RequestEndpointSet<()> for Routes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[("/test", RequestEndpointPolicy::AllowList(&[]))];

        async fn dispatch(
            _cx: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            Err(Decline::Ignore)
        }
    }

    fn configured_engine(
        request_endpoints: ServeMyRequestEndpoints,
        maximum_request_bytes: ByteLimit,
    ) -> (EngineState<Storage>, DestinationHash) {
        let mut engine = EngineState::<Storage>::default();
        let destination = configure_preconfigured_destination::<(), Routes, Storage>(
            &mut engine,
            PreConfiguredDestination::Single {
                app_name: "test",
                aspects: &["requests"],
                identity: Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]),
                announce_app_data: &[],
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes,
                request_endpoints,
            },
        )
        .expect("the test destination fits fixed storage");
        (engine, destination)
    }

    #[test]
    fn node_route_set_attaches_routes_to_the_destination() {
        let (mut engine, destination) =
            configured_engine(ServeMyRequestEndpoints::Yes, ByteLimit::Unlimited);

        assert_eq!(
            engine.allow_requester(&destination, "/test", IdentityHash::new([0x22; 16])),
            Ok(())
        );
    }

    #[test]
    fn none_leaves_routes_unattached_from_the_destination() {
        let (mut engine, destination) =
            configured_engine(ServeMyRequestEndpoints::No, ByteLimit::Unlimited);

        assert_eq!(
            engine.allow_requester(&destination, "/test", IdentityHash::new([0x22; 16])),
            Err(RequestHandlerError::NoSuchHandler)
        );
    }

    #[test]
    fn recipe_request_limit_reaches_the_registered_destination() {
        let (engine, destination) =
            configured_engine(ServeMyRequestEndpoints::No, ByteLimit::Maximum(1_024));

        assert_eq!(
            engine
                .upstream_app_destinations()
                .find(|registered| registered.destination == destination)
                .and_then(|registered| match registered.kind {
                    crate::routing::upstream_app_destinations::UpstreamAppDestinationKind::Single {
                        maximum_request_bytes,
                        ..
                    } => Some(maximum_request_bytes),
                    crate::routing::upstream_app_destinations::UpstreamAppDestinationKind::Plain
                    | crate::routing::upstream_app_destinations::UpstreamAppDestinationKind::Group => None,
                }),
            Some(ByteLimit::Maximum(1_024)),
        );
    }

    #[test]
    fn in_place_assembly_initializes_and_configures_the_node() {
        let mut slot = MaybeUninit::uninit();
        let storage: Storage = TestFixedStorage;
        let (node, ManuallyAttached, NoPersistence) = assemble_node_in_place(
            &mut slot,
            PrnsNodeRecipe {
                transport_identity: Some(Zeroizing::new([0x33; IDENTITY_SECRET_KEY_LEN])),
                pre_configured_destinations: [PreConfiguredDestination::Plain {
                    app_name: "test",
                    aspects: &["plain"],
                }],
                app_state: (),
                storage,
                request_endpoints: (),
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_, _| {},
            },
        );

        assert!(node.engine.network_transport_enabled());
        assert_eq!(node.engine.held_identity_hashes().len(), 1);
        assert_eq!(node.engine.upstream_app_destinations().count(), 1);
    }

    #[test]
    fn group_recipe_registers_its_address_and_shared_key_together() {
        let mut engine = EngineState::<Storage>::default();
        let identity = IdentityHash::new([0x44; 16]);
        let configured = configure_preconfigured_destination::<(), (), Storage>(
            &mut engine,
            PreConfiguredDestination::Group {
                app_name: "test",
                aspects: &["group"],
                identity,
                shared_key: &[0x42; 64],
            },
        )
        .unwrap();
        let expected = PreConfiguredDestination::Group {
            app_name: "test",
            aspects: &["group"],
            identity,
            shared_key: &[0x42; 64],
        }
        .destination_hash()
        .unwrap();
        assert_eq!(configured, expected);
        assert_eq!(
            engine.ingest_send_group(
                crate::engine::CommandId(9),
                crate::engine::SendGroup {
                    destination: configured,
                    payload: crate::engine::SendGroupPayload::new(),
                }
            ),
            crate::engine::CommandOutcome::OwesSendGroup {
                id: crate::engine::CommandId(9),
                send: crate::engine::SendGroup {
                    destination: configured,
                    payload: crate::engine::SendGroupPayload::new(),
                },
            }
        );
    }

    #[test]
    #[should_panic(expected = "no destination serves them")]
    fn declared_endpoints_with_no_serving_destination_fail_loudly() {
        let mut slot = MaybeUninit::uninit();
        let storage: Storage = TestFixedStorage;
        let (_node, ManuallyAttached, NoPersistence) = assemble_node_in_place(
            &mut slot,
            PrnsNodeRecipe {
                transport_identity: None,
                pre_configured_destinations: [PreConfiguredDestination::Plain {
                    app_name: "test",
                    aspects: &["plain"],
                }],
                app_state: (),
                storage,
                request_endpoints: Routes,
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_, _| {},
            },
        );
    }
}
