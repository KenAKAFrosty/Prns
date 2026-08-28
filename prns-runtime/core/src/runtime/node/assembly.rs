use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::engine::EngineState;
use crate::engine::RatchetPolicy;
use crate::identity::held::HoldIdentityError;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::remote_control::{
    FixedRemoteControlAccessTable, RemoteControlAccessTable, RemoteControlAnnouncementData,
    RemoteControlAnnouncementDataWriteError, RemoteControlEndpoint, RemoteControlNodeIdentities,
    RemoteControlRequest, RemoteControlRequestSet, RemoteControlSelfAnnouncement,
    RemoteControlService, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
    DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS, REMOTE_CONTROL_APPLICATION_ASPECTS,
    REMOTE_CONTROL_APPLICATION_NAME, REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash, RequestPolicy};
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
    pub remote_control: AssembledRemoteControl,
    pub state: St,
    pub on_event: F,
    pub request_endpoints: PhantomData<R>,
}

#[derive(Debug)]
pub struct AssembledRemoteControl {
    identities: RemoteControlNodeIdentities,
    target_endpoint: RemoteControlEndpoint,
    request_endpoint_id: RequestPathHash,
    access: FixedRemoteControlAccessTable<DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS>,
    available_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
}

impl AssembledRemoteControl {
    #[must_use]
    pub const fn identities(&self) -> &RemoteControlNodeIdentities {
        &self.identities
    }

    #[must_use]
    pub const fn target_endpoint(&self) -> RemoteControlEndpoint {
        self.target_endpoint
    }

    #[must_use]
    pub const fn request_endpoint_id(&self) -> RequestPathHash {
        self.request_endpoint_id
    }

    #[must_use]
    pub const fn access(
        &self,
    ) -> &FixedRemoteControlAccessTable<DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS> {
        &self.access
    }

    #[must_use]
    pub const fn self_announcement(&self) -> RemoteControlSelfAnnouncement {
        self.self_announcement
    }

    #[must_use]
    pub const fn available_requests(&self) -> RemoteControlRequestSet {
        self.available_requests
    }

    pub fn set_controller_grant(
        &mut self,
        grant: crate::remote_control::RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError> {
        self.access.set_controller_grant(grant)
    }

    pub fn revoke_controller(
        &mut self,
        controller: &crate::remote_control::RemoteControlControllerIdentity,
    ) -> RevokeRemoteControlControllerOutcome {
        self.access.revoke_controller(controller)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureRemoteControlServiceError {
    HoldIdentity(HoldIdentityError),
    EncodeAnnouncementData(RemoteControlAnnouncementDataWriteError),
    RegisterTarget(RegisterDestinationError),
    ConfigureRequestLimit,
    RegisterRequestEndpoint(TablePushError),
    BuildAccess(TablePushError),
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

pub fn configure_remote_control_service<S>(
    engine: &mut EngineState<S>,
    service: RemoteControlService<'_>,
) -> Result<AssembledRemoteControl, ConfigureRemoteControlServiceError>
where
    S: StorageLayout,
{
    let available_requests = service.available_requests();
    let (identity_secrets, default_public_app_data, initial_access, self_announcement) =
        service.into_parts();
    let identities = engine
        .configure_remote_control_identities(identity_secrets)
        .map_err(ConfigureRemoteControlServiceError::HoldIdentity)?;
    let target_endpoint = identities.target().endpoint();
    let announcement_data = RemoteControlAnnouncementData::new(default_public_app_data)
        .encode()
        .map_err(ConfigureRemoteControlServiceError::EncodeAnnouncementData)?;
    let destination = engine
        .register_single_destination(
            &identities.target().identity_hash(),
            REMOTE_CONTROL_APPLICATION_NAME,
            REMOTE_CONTROL_APPLICATION_ASPECTS,
            announcement_data.as_slice(),
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .map_err(ConfigureRemoteControlServiceError::RegisterTarget)?;
    if !engine.set_maximum_request_bytes(
        &destination,
        ByteLimit::Maximum(RemoteControlRequest::MAX_ENCODED_LEN as u64),
    ) {
        return Err(ConfigureRemoteControlServiceError::ConfigureRequestLimit);
    }
    let request_endpoint_id = RequestPathHash::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID);
    engine
        .register_request_handler_hash(
            &destination,
            request_endpoint_id,
            RequestPolicy::RequireIdentified,
        )
        .map_err(ConfigureRemoteControlServiceError::RegisterRequestEndpoint)?;
    let mut access = FixedRemoteControlAccessTable::default();
    for grant in initial_access.grants() {
        access
            .upsert(*grant)
            .map_err(ConfigureRemoteControlServiceError::BuildAccess)?;
    }
    Ok(AssembledRemoteControl {
        identities,
        target_endpoint,
        request_endpoint_id,
        access,
        available_requests,
        self_announcement,
    })
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
    recipe: PrnsNodeRecipe<'a, D, St, R, F, I, S, P>,
) -> (AssembledNode<St, R, F, S>, I, P)
where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let PrnsNodeRecipe {
        transport_identity,
        remote_control,
        pre_configured_destinations,
        app_state,
        storage: _,
        request_endpoints: _,
        interfaces,
        persistence,
        on_event,
    } = recipe;

    let mut engine = EngineState::<S>::default();
    let remote_control = configure_remote_control_service(&mut engine, remote_control)
        .expect("the RemoteControl service fits the node storage");
    let mut node = AssembledNode {
        engine,
        remote_control,
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
#[allow(clippy::expect_used)]
pub fn assemble_node_in_place<'a, 'slot, D, St, R, F, I, S, P>(
    slot: &'slot mut MaybeUninit<AssembledNode<St, R, F, S>>,
    recipe: PrnsNodeRecipe<'a, D, St, R, F, I, S, P>,
) -> (&'slot mut AssembledNode<St, R, F, S>, I, P)
where
    D: IntoIterator<Item = PreConfiguredDestination<'a>>,
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    let PrnsNodeRecipe {
        transport_identity,
        remote_control,
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
        let engine = &mut *core::ptr::addr_of_mut!((*node).engine);
        let remote_control = configure_remote_control_service(engine, remote_control)
            .expect("the RemoteControl service fits the node storage");
        core::ptr::addr_of_mut!((*node).remote_control).write(remote_control);
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
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use crate::remote_control::{
        RemoteControlControllerGrant, RemoteControlControllerGrants,
        RemoteControlControllerIdentity, RemoteControlControllerIdentitySecret,
        RemoteControlInitialAccess, RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData,
        RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlTargetIdentitySecret,
        REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY,
    };
    use crate::routing::request_handlers::RequestPathHash;
    use crate::engine::RatchetPolicy;
    use crate::routing::links::resources::ResourceStrategy;
    use crate::routing::{LinkRequestPolicy, ProofStrategy};
    use crate::runtime::request_endpoints::{Decline, RequestContext, RequestEndpointPolicy};
    use crate::runtime::{ManuallyAttached, NoPersistence, ServeMyRequestEndpoints};
    use crate::storage::TestFixedStorage;

    type Storage = TestFixedStorage<4, 4, 128, 4, 4, 4, 2, 2, 2, 2, 2, 2>;

    struct Routes;

    fn remote_control_identity_secrets(
        controller_fill: u8,
        target_fill: u8,
    ) -> RemoteControlNodeIdentitySecrets {
        RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [controller_fill; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [target_fill; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap()
    }

    fn remote_control_controller(fill: u8) -> RemoteControlControllerIdentity {
        *remote_control_identity_secrets(fill, fill.saturating_add(1))
            .identities()
            .controller()
    }

    fn remote_control_grant(fill: u8) -> RemoteControlControllerGrant {
        RemoteControlControllerGrant::new(
            remote_control_controller(fill),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap()
    }

    fn remote_control_service() -> RemoteControlService<'static> {
        RemoteControlService::new(
            remote_control_identity_secrets(0x71, 0x72),
            RemoteControlPublicAppData::try_from(b"".as_slice()).unwrap(),
            RemoteControlInitialAccess::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        )
    }

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
    fn remote_control_service_registers_its_independent_target_and_access() {
        let mut engine = EngineState::<Storage>::default();
        let identity_secrets = remote_control_identity_secrets(0x31, 0x32);
        let expected_identities = identity_secrets.identities();
        let grants = [remote_control_grant(0x41), remote_control_grant(0x51)];
        let initial_access = RemoteControlInitialAccess::Grants(
            RemoteControlControllerGrants::try_from(grants.as_slice()).unwrap(),
        );
        let service = RemoteControlService::new(
            identity_secrets,
            RemoteControlPublicAppData::try_from(b"node".as_slice()).unwrap(),
            initial_access,
            RemoteControlSelfAnnouncement::Destination(DestinationHash::new([0x61; 16])),
        );

        let configured = configure_remote_control_service(&mut engine, service).unwrap();
        let target_endpoint = configured.target_endpoint();
        let target_destination = target_endpoint.destination_hash();

        assert_eq!(configured.identities(), &expected_identities);
        assert_eq!(configured.access().grants(), grants.as_slice());
        assert_eq!(engine.transport_id(), None);
        assert_eq!(engine.held_identity_hashes().len(), 2);
        assert_eq!(
            engine
                .upstream_app_destinations()
                .find(|registered| registered.destination == target_destination)
                .map(|registered| registered.kind),
            Some(
                crate::routing::upstream_app_destinations::UpstreamAppDestinationKind::Single {
                    identity: expected_identities.target().identity_hash(),
                    proof_strategy: ProofStrategy::ProveAll,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: ByteLimit::Maximum(
                        RemoteControlRequest::MAX_ENCODED_LEN as u64,
                    ),
                    ratchet_policy: RatchetPolicy::NoRatchets,
                },
            ),
        );
    }

    #[test]
    fn assembled_remote_control_owns_controller_grant_changes() {
        let mut engine = EngineState::<Storage>::default();
        let mut remote_control =
            configure_remote_control_service(&mut engine, remote_control_service()).unwrap();
        let initial = remote_control_grant(0x41);
        let updated = RemoteControlControllerGrant::new(
            *initial.controller(),
            RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
        )
        .unwrap();

        assert_eq!(
            remote_control.set_controller_grant(initial),
            Ok(SetRemoteControlControllerGrantOutcome::Added),
        );
        assert_eq!(
            remote_control.set_controller_grant(initial),
            Ok(SetRemoteControlControllerGrantOutcome::Unchanged),
        );
        assert_eq!(
            remote_control.set_controller_grant(updated),
            Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: initial }),
        );
        assert_eq!(remote_control.access().grants(), &[updated]);
        assert_eq!(
            remote_control.revoke_controller(updated.controller()),
            RevokeRemoteControlControllerOutcome::Revoked { grant: updated },
        );
        assert_eq!(
            remote_control.revoke_controller(updated.controller()),
            RevokeRemoteControlControllerOutcome::NotFound,
        );
        assert!(remote_control.access().is_empty());
    }

    #[test]
    fn hopspot_style_recipe_needs_three_held_identity_rows_with_remote_control() {
        let tight_storage = TestFixedStorage::<4, 4, 128, 4, 2, 2, 2, 2, 2, 2, 2, 2>;
        let roomy_storage = TestFixedStorage::<4, 4, 128, 4, 3, 2, 2, 2, 2, 2, 2, 2>;
        let node_secret = Zeroizing::new([0x55; IDENTITY_SECRET_KEY_LEN]);
        let destinations = || {
            [
                PreConfiguredDestination::Single {
                    app_name: "lxmf",
                    aspects: &["delivery"],
                    identity: node_secret.clone(),
                    announce_app_data: b"delivery",
                    proof: ProofStrategy::ProveAll,
                    link_requests: LinkRequestPolicy::AcceptAll,
                    ratchet: RatchetPolicy::Ratcheted,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: Default::default(),
                    request_endpoints: ServeMyRequestEndpoints::No,
                },
                PreConfiguredDestination::Single {
                    app_name: "node",
                    aspects: &["page"],
                    identity: node_secret.clone(),
                    announce_app_data: b"node",
                    proof: ProofStrategy::ProveNone,
                    link_requests: LinkRequestPolicy::AcceptAll,
                    ratchet: RatchetPolicy::NoRatchets,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: Default::default(),
                    request_endpoints: ServeMyRequestEndpoints::Yes,
                },
            ]
        };
        let mut tight = MaybeUninit::uninit();
        let tight_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assemble_node_in_place(
                &mut tight,
                PrnsNodeRecipe {
                    transport_identity: Some(node_secret.clone()),
                    remote_control: remote_control_service(),
                    pre_configured_destinations: destinations(),
                    app_state: (),
                    storage: tight_storage,
                    request_endpoints: (),
                    interfaces: ManuallyAttached,
                    persistence: NoPersistence,
                    on_event: |_, _| {},
                },
            );
        }));
        assert!(tight_result.is_err(), "two held rows cannot fit RC + Hopspot");

        let mut roomy = MaybeUninit::uninit();
        let (node, _, _) = assemble_node_in_place(
            &mut roomy,
            PrnsNodeRecipe {
                transport_identity: Some(node_secret.clone()),
                remote_control: remote_control_service(),
                pre_configured_destinations: destinations(),
                app_state: (),
                storage: roomy_storage,
                request_endpoints: (),
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_, _| {},
            },
        );
        assert_eq!(node.engine.held_identity_hashes().len(), 3);
    }

    #[test]
    fn hopspot_style_recipe_needs_three_upstream_rows_with_remote_control_on_esp32s3() {
        type Esp32LikeStorage = TestFixedStorage<4, 4, 128, 2, 3, 2, 2, 2, 2, 2, 2, 2>;
        let node_secret = Zeroizing::new([0x55; IDENTITY_SECRET_KEY_LEN]);
        let destinations = || {
            [
                PreConfiguredDestination::Single {
                    app_name: "lxmf",
                    aspects: &["delivery"],
                    identity: node_secret.clone(),
                    announce_app_data: b"delivery",
                    proof: ProofStrategy::ProveAll,
                    link_requests: LinkRequestPolicy::AcceptAll,
                    ratchet: RatchetPolicy::Ratcheted,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: Default::default(),
                    request_endpoints: ServeMyRequestEndpoints::No,
                },
                PreConfiguredDestination::Single {
                    app_name: "node",
                    aspects: &["page"],
                    identity: node_secret.clone(),
                    announce_app_data: b"node",
                    proof: ProofStrategy::ProveNone,
                    link_requests: LinkRequestPolicy::AcceptAll,
                    ratchet: RatchetPolicy::NoRatchets,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: Default::default(),
                    request_endpoints: ServeMyRequestEndpoints::Yes,
                },
            ]
        };
        let storage: Esp32LikeStorage = TestFixedStorage;
        let mut slot = MaybeUninit::uninit();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assemble_node_in_place(
                &mut slot,
                PrnsNodeRecipe {
                    transport_identity: Some(node_secret.clone()),
                    remote_control: remote_control_service(),
                    pre_configured_destinations: destinations(),
                    app_state: (),
                    storage,
                    request_endpoints: (),
                    interfaces: ManuallyAttached,
                    persistence: NoPersistence,
                    on_event: |_, _| {},
                },
            );
        }));
        assert!(
            result.is_err(),
            "two upstream rows cannot fit RC target + delivery + node page"
        );

        type Esp32FixedStorage = TestFixedStorage<4, 4, 128, 3, 3, 2, 2, 2, 2, 2, 2, 2>;
        let storage: Esp32FixedStorage = TestFixedStorage;
        let mut slot = MaybeUninit::uninit();
        let (node, _, _) = assemble_node_in_place(
            &mut slot,
            PrnsNodeRecipe {
                transport_identity: Some(node_secret.clone()),
                remote_control: remote_control_service(),
                pre_configured_destinations: destinations(),
                app_state: (),
                storage,
                request_endpoints: (),
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_, _| {},
            },
        );
        assert_eq!(node.engine.upstream_app_destinations().count(), 3);
    }

    #[test]
    fn in_place_assembly_initializes_and_configures_the_node() {
        let mut slot = MaybeUninit::uninit();
        let storage: Storage = TestFixedStorage;
        let (node, ManuallyAttached, NoPersistence) = assemble_node_in_place(
            &mut slot,
            PrnsNodeRecipe {
                transport_identity: Some(Zeroizing::new([0x33; IDENTITY_SECRET_KEY_LEN])),
                remote_control: remote_control_service(),
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
        assert_eq!(
            node.engine.held_identity_hashes().len(),
            REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY.saturating_add(1),
        );
        assert_eq!(node.engine.upstream_app_destinations().count(), 2);
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
                remote_control: remote_control_service(),
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
