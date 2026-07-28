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
use crate::wire::DestinationHash;

use super::super::request_endpoints::RequestEndpointSet;
use super::super::PrnsEvent;
use super::recipe::{PreConfiguredDestination, PrnsNodeRecipe, RequestEndpointRegistration};

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
        PreConfiguredDestination::Single {
            app_name,
            aspects,
            identity,
            announce_app_data,
            proof,
            link_requests,
            ratchet,
            resource_strategy,
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
            },
            request_endpoints,
        ),
    }
}

fn configure_single_destination<St, R, S>(
    engine: &mut EngineState<S>,
    configuration: SingleDestinationConfiguration<'_>,
    request_endpoints: RequestEndpointRegistration,
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
    if matches!(
        request_endpoints,
        RequestEndpointRegistration::NodeRequestEndpointSet
    ) {
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
pub fn assemble_node<'a, D, St, R, F, I, S>(
    recipe: PrnsNodeRecipe<D, St, R, F, I, S>,
) -> (AssembledNode<St, R, F, S>, I)
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
        on_event,
    } = recipe;

    let mut node = AssembledNode {
        engine: EngineState::<S>::default(),
        state: app_state,
        on_event,
        request_endpoints: PhantomData,
    };
    configure_assembled_node(&mut node, pre_configured_destinations, transport_identity);
    (node, interfaces)
}

#[expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "every AssembledNode field is initialized before the slot is exposed"
)]
pub fn assemble_node_in_place<'a, 'slot, D, St, R, F, I, S>(
    slot: &'slot mut MaybeUninit<AssembledNode<St, R, F, S>>,
    recipe: PrnsNodeRecipe<D, St, R, F, I, S>,
) -> (&'slot mut AssembledNode<St, R, F, S>, I)
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
    (node, interfaces)
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
    for destination in pre_configured_destinations {
        configure_preconfigured_destination::<St, R, S>(&mut node.engine, destination)
            .expect("recipe destination is valid and fits the store");
    }

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
    use crate::runtime::ManuallyAttached;
    use crate::storage::TestFixedStorage;

    type Storage = TestFixedStorage<4, 4, 128, 4, 4, 4, 2, 2, 2, 2, 2, 2>;

    struct Routes;

    impl RequestEndpointSet<()> for Routes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[("/test", RequestEndpointPolicy::AllowList(&[]))];

        async fn dispatch(
            _cx: RequestContext<'_, ()>,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            Err(Decline::Ignore)
        }
    }

    fn configured_engine(
        request_endpoints: RequestEndpointRegistration,
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
                request_endpoints,
            },
        )
        .expect("the test destination fits fixed storage");
        (engine, destination)
    }

    #[test]
    fn node_route_set_attaches_routes_to_the_destination() {
        let (mut engine, destination) =
            configured_engine(RequestEndpointRegistration::NodeRequestEndpointSet);

        assert_eq!(
            engine.allow_requester(&destination, "/test", IdentityHash::new([0x22; 16])),
            Ok(())
        );
    }

    #[test]
    fn none_leaves_routes_unattached_from_the_destination() {
        let (mut engine, destination) = configured_engine(RequestEndpointRegistration::None);

        assert_eq!(
            engine.allow_requester(&destination, "/test", IdentityHash::new([0x22; 16])),
            Err(RequestHandlerError::NoSuchHandler)
        );
    }

    #[test]
    fn in_place_assembly_initializes_and_configures_the_node() {
        let mut slot = MaybeUninit::uninit();
        let storage: Storage = TestFixedStorage;
        let (node, ManuallyAttached) = assemble_node_in_place(
            &mut slot,
            PrnsNodeRecipe {
                transport_identity: Some(Zeroizing::new([0x33; IDENTITY_SECRET_KEY_LEN])),
                pre_configured_destinations: [PreConfiguredDestination::Plain {
                    app_name: "test",
                    aspects: &["plain"],
                }],
                app_state: (),
                storage,
                request_endpoints: Routes,
                interfaces: ManuallyAttached,
                on_event: |_, _| {},
            },
        );

        assert!(node.engine.network_transport_enabled());
        assert_eq!(node.engine.held_identity_hashes().len(), 1);
        assert_eq!(node.engine.upstream_app_destinations().count(), 1);
    }
}
