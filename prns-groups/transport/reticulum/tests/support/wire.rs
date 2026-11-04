//! Controlled carrier only: every delivered byte still traverses a real node,
//! path discovery, authenticated Link, request codec and Resource state machine.
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use prns_groups_reticulum::{
    destination_config, enrollment, Ingress, Limits, Network, Peer, RequestJob,
};
use prns_host::{
    DestinationConfig, HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits,
};
use prns_host_runtime::owner::OwnedSession;
use prns_host_runtime::service_scope::HostServiceScope;
use prns_host_runtime::{ApplicationEventDispatch, HostEmbedding, HostPreparedAttachment};
use tokio::sync::mpsc;

#[path = "link_wire.rs"]
mod link_wire;
pub type WireState = link_wire::WireState<PEERS>;

pub const PEERS: usize = 4;

pub fn limits() -> Limits {
    Limits {
        // The native fixture's 64 KiB MLS envelope also needs its sync wrapper.
        max_frame_bytes: 131_072,
        max_page_items: 8,
        max_peers: PEERS,
        inbound_jobs: 16,
        exchange_timeout: Duration::from_secs(5),
    }
}

pub fn enrollment_limits() -> enrollment::Limits {
    enrollment::Limits {
        max_frame_bytes: enrollment::MAX_FRAME_BYTES,
        inbound_jobs: 4,
        exchange_timeout: Duration::from_secs(5),
    }
}

pub struct Nodes {
    _host_slots: tokio::sync::OwnedSemaphorePermit,
    pub owners: Vec<Arc<OwnedSession>>,
    #[allow(dead_code)] // Used by the scoped enrollment integration test.
    pub scopes: Vec<HostServiceScope>,
    pub networks: Vec<Arc<Network>>,
    pub jobs: Vec<Option<mpsc::Receiver<RequestJob>>>,
    #[allow(dead_code)] // Used by the enrollment integration binary.
    pub enrollment_networks: Vec<Arc<enrollment::Network>>,
    #[allow(dead_code)] // Used by the enrollment integration binary.
    pub enrollment_jobs: Vec<Option<mpsc::Receiver<RequestJob>>>,
    pub state: Arc<WireState>,
    forwarder: tokio::task::JoinHandle<()>,
}
impl Nodes {
    pub async fn open() -> Self {
        Self::open_impl(false, limits(), enrollment_limits()).await
    }
    #[allow(dead_code)] // Used by the scoped enrollment integration test.
    pub async fn open_scoped() -> Self {
        Self::open_impl(true, limits(), enrollment_limits()).await
    }
    /// Explicit scale-fixture bounds; ordinary four-host fixtures retain defaults.
    #[allow(dead_code)]
    pub async fn open_scoped_with_limits(
        sync_limits: Limits,
        enrollment_limits: enrollment::Limits,
    ) -> Self {
        Self::open_impl(true, sync_limits, enrollment_limits).await
    }
    async fn open_impl(
        scoped: bool,
        sync_limits: Limits,
        enrollment_limits: enrollment::Limits,
    ) -> Self {
        // Real host actors use a process-wide bounded registry. Keep concurrent
        // test fixtures below that registry instead of racing unrelated tests.
        static FIXTURES: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        let host_slots = FIXTURES
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let (interfaces, state, forward) = link_wire::prepare::<PEERS>();
        let mut owners = Vec::new();
        let mut scopes = Vec::new();
        let mut jobs = Vec::new();
        let mut enrollment_jobs = Vec::new();
        for (peer, interface) in interfaces.into_iter().enumerate() {
            let ingress = Arc::new(Mutex::new(None::<Ingress>));
            let callback = Arc::clone(&ingress);
            let enrollment_ingress = Arc::new(Mutex::new(None::<enrollment::Ingress>));
            let enrollment_callback = Arc::clone(&enrollment_ingress);
            let callback_state = Arc::clone(&state);
            let scope_events = Arc::new(Mutex::new(None::<HostServiceScope>));
            let callback_scope = scope_events.clone();
            let mut sync_config = destination_config(sync_limits).unwrap();
            let mut enrollment_config = enrollment::destination_config(enrollment_limits).unwrap();
            let secret = [peer as u8 + 0x60; 64];
            if scoped {
                sync_config.identity = prns_host::DestinationIdentityConfig::Dedicated(
                    IdentityConfig::Existing(prns_host::IdentitySecret::new(secret)),
                );
                enrollment_config.identity = prns_host::DestinationIdentityConfig::Dedicated(
                    IdentityConfig::Existing(prns_host::IdentitySecret::new(secret)),
                );
            }
            let owner = OwnedSession::open_with_embedding(
                HostConfig {
                    identity: IdentityConfig::GenerateEphemeral,
                    persistence: PersistenceConfig::Ephemeral,
                    role: HostRole::Endpoint,
                    destinations: vec![
                        DestinationConfig::Single(sync_config),
                        DestinationConfig::Single(enrollment_config),
                    ],
                    required_capabilities: vec![],
                    limits: PrnsLimits::balanced(),
                },
                HostEmbedding {
                    on_event: Some(Box::new(move |event| {
                        if let Some(scope) = callback_scope.lock().unwrap().as_ref() {
                            scope.on_event_ref(&event);
                        }
                        if let Some(ingress) = callback.lock().unwrap().as_ref() {
                            if ingress.on_event_ref(&event)
                                == prns_groups_reticulum::IngressOutcome::Queued
                            {
                                callback_state.jobs[peer].fetch_add(1, Ordering::SeqCst);
                            }
                        }
                        if let Some(ingress) = enrollment_callback.lock().unwrap().as_ref() {
                            ingress.on_event_ref(&event);
                        }
                    })),
                    application_events: ApplicationEventDispatch::EmbeddingCallback,
                    prepare_interfaces: Some(Box::new(move |client| {
                        Ok(vec![HostPreparedAttachment::Interface {
                            attachment: client.protocols().attach(interface),
                            kind: prns_host::InterfaceKind::Pipe,
                        }])
                    })),
                    ..HostEmbedding::default()
                },
            )
            .await
            .unwrap();
            let destination = owner.client().destination_hashes().unwrap()[0];
            let identity = if scoped {
                prns_host::IdentityHash::new(
                    *personal_rns::identity::PrivateIdentityMaterial::from(
                        personal_rns::identity::vault::IdentitySecretKey::new(secret),
                    )
                    .identity_hash()
                    .as_bytes(),
                )
            } else {
                owner.client().identity_hash().unwrap()
            };
            let scope = HostServiceScope::new(
                owner.client(),
                identity,
                owner.client().destination_hashes().unwrap(),
                16,
                8,
            )
            .unwrap();
            *scope_events.lock().unwrap() = Some(scope.clone());
            scopes.push(scope);
            let (callback, receiver) = Ingress::new(destination, sync_limits).unwrap();
            *ingress.lock().unwrap() = Some(callback);
            let (callback, receiver_enrollment) = enrollment::Ingress::new(
                enrollment::destination_for(identity).unwrap(),
                enrollment_limits,
            )
            .unwrap();
            *enrollment_ingress.lock().unwrap() = Some(callback);
            enrollment_jobs.push(Some(receiver_enrollment));
            owners.push(owner);
            jobs.push(Some(receiver));
        }
        let forwarder = tokio::spawn(forward);
        let peers: Vec<_> = owners
            .iter()
            .enumerate()
            .map(|(index, owner)| Peer {
                endpoint: super::endpoint(index),
                identity: scopes[index].identity_hash(),
                destination: owner.client().destination_hashes().unwrap()[0],
            })
            .collect();
        let mut networks = Vec::new();
        let mut enrollment_networks = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            // The fourth endpoint may know the members, but members do not trust it.
            let network = if scoped {
                Network::scoped(
                    scopes[index].clone(),
                    peers[index].destination,
                    peers[..3].to_vec(),
                    sync_limits,
                )
            } else {
                Network::new(
                    owner.client(),
                    peers[index].destination,
                    peers[..3].to_vec(),
                    sync_limits,
                )
            }
            .unwrap();
            network.authorize_peers().await.unwrap();
            networks.push(Arc::new(network));
            enrollment_networks.push(Arc::new(
                if scoped {
                    enrollment::Network::scoped(
                        scopes[index].clone(),
                        enrollment::destination_for(scopes[index].identity_hash()).unwrap(),
                        enrollment_limits,
                    )
                } else {
                    enrollment::Network::new(
                        owner.client(),
                        enrollment::destination_for(scopes[index].identity_hash()).unwrap(),
                        enrollment_limits,
                    )
                }
                .unwrap(),
            ));
        }
        Self {
            _host_slots: host_slots,
            owners,
            scopes,
            networks,
            jobs,
            enrollment_networks,
            enrollment_jobs,
            state,
            forwarder,
        }
    }
    pub async fn stop(self) {
        for owner in self.owners {
            owner.stop().await.unwrap();
        }
        self.forwarder.abort();
    }
}
