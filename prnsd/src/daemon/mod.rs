mod background;
pub(crate) mod configuration;
mod configured_interfaces;
mod identity;
mod interface_failure;
mod interface_ownership;

use personal_rns::runtime::NoPersistence;

pub(crate) use configured_interfaces::{
    construct as construct_configured_interfaces, AttachedConfiguredInterface,
};

pub(crate) use configuration::DEFAULT_CONFIG;

use std::future;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::time::Duration;

use crate::shutdown::ShutdownSignal;
use crate::{cli, interface_discovery, nnpages, observability, persistence, services, splash};
use personal_rns::browser_rendezvous::{AutoWifiDevicePolicy, BrowserRendezvous};
use personal_rns::config::{SharedInstance, TransportIdentityPolicy};
use personal_rns::engine::{
    EngineProtocolPolicy, LinkMtuDiscovery, LocalHopCountOverride, ProofForm,
    RecursivePathRequestDefault,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::interfaces::ConnectionState;
use personal_rns::node_introspection::logical_interface_inventory;
use personal_rns::remote_control::{
    RemoteControlInitialAccess, RemoteControlPublicAppData, RemoteControlSelfAnnouncement,
    RemoteControlService,
};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::runtime::{
    wall_clock_timeline_origin, CryptoPoolConfig, Diagnostic, ManuallyAttached, NodePersistence,
    NodeRunError, PersistenceFlushStatus, PoolWorkers, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    RemoteControlFileIdentityBootstrapError,
};
use personal_rns::shared_instance::{RnsBlackholeFiles, SharedInstanceCredentials};
use personal_rns::storage::GrowableHeap;
use personal_rns::PlanRuntimeContext;
use prnsd_control::{config_digest, ManagedProcess, ReloadRequest, ReloadResult, ServiceError};

const TRAY_STATUS_INTERVAL: Duration = Duration::from_secs(2);
const NNPAGES_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DaemonStatus {
    pub(crate) interface_count: u32,
    pub(crate) retrying: u32,
    pub(crate) impaired: u32,
    pub(crate) unavailable: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonReady {
    pub(crate) config_dir: PathBuf,
    pub(crate) managed_state_dir: Option<PathBuf>,
    pub(crate) status: DaemonStatus,
}

#[derive(Clone)]
pub(crate) struct DaemonStatusPublisher {
    publish: Arc<dyn Fn(DaemonStatus) + Send + Sync>,
}

impl DaemonStatusPublisher {
    #[cfg(feature = "tray")]
    pub(crate) fn new(publish: impl Fn(DaemonStatus) + Send + Sync + 'static) -> Self {
        Self {
            publish: Arc::new(publish),
        }
    }

    fn publish(&self, status: DaemonStatus) {
        (self.publish)(status);
    }
}

pub(crate) struct DaemonPresentation {
    pub(crate) ready: tokio::sync::oneshot::Sender<DaemonReady>,
    pub(crate) status: DaemonStatusPublisher,
}

#[derive(Debug)]
pub(crate) enum DaemonRunError {
    TransportIdentityUnavailable(personal_rns::runtime::IdentitySecretFileError),
    RemoteControlIdentityUnavailable(RemoteControlFileIdentityBootstrapError),
    RemoteControlSelfAnnouncementUnavailable(ExpandNameError),
    PersistenceUnavailable(std::io::Error),
    PersistenceFlushFailed,
    PersistenceWorkerStopped,
    NodeStopped,
    NodePanicked(NodeRunError),
    InterfaceFailed,
}

impl core::fmt::Display for DaemonRunError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TransportIdentityUnavailable(error) => {
                write!(
                    formatter,
                    "required transport identity is unavailable: {error}"
                )
            }
            Self::RemoteControlIdentityUnavailable(error) => {
                write!(
                    formatter,
                    "required RemoteControl identities are unavailable: {error}"
                )
            }
            Self::RemoteControlSelfAnnouncementUnavailable(error) => {
                write!(
                    formatter,
                    "RemoteControl self-announcement is unavailable: {error:?}"
                )
            }
            Self::PersistenceUnavailable(error) => {
                write!(formatter, "required persistence is unavailable: {error}")
            }
            Self::PersistenceFlushFailed => {
                formatter.write_str("required persistence failed to flush")
            }
            Self::PersistenceWorkerStopped => {
                formatter.write_str("persistence worker stopped before node shutdown")
            }
            Self::NodeStopped => formatter.write_str("node stopped unexpectedly"),
            Self::NodePanicked(error) => write!(formatter, "node callback panicked: {error:?}"),
            Self::InterfaceFailed => formatter.write_str("an interface failed fatally"),
        }
    }
}

impl std::error::Error for DaemonRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TransportIdentityUnavailable(error) => Some(error),
            Self::RemoteControlIdentityUnavailable(error) => Some(error),
            Self::PersistenceUnavailable(error) => Some(error),
            Self::RemoteControlSelfAnnouncementUnavailable(_)
            | Self::PersistenceFlushFailed
            | Self::PersistenceWorkerStopped
            | Self::NodeStopped
            | Self::NodePanicked(_)
            | Self::InterfaceFailed => None,
        }
    }
}

pub(super) async fn run(
    cli: cli::DaemonArgs,
    managed: Option<ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
    presentation: Option<DaemonPresentation>,
) -> Result<(), DaemonRunError> {
    let operating_system_shutdown = persistence::capture_operating_system_shutdown();
    let (ready, status_publisher) = match presentation {
        Some(presentation) => (Some(presentation.ready), Some(presentation.status)),
        None => (None, None),
    };
    #[cfg(all(feature = "tray", target_os = "linux"))]
    let mut status_publisher = status_publisher;
    let started = std::time::Instant::now();
    let configuration::LoadedConfiguration {
        directory: config_dir,
        path: config_path,
        plan,
        warnings: config_warnings,
    } = configuration::load_or_exit(cli.config.as_deref(), cli.bootstrap);
    let observability = match observability::init(cli.log_format, plan.logging) {
        Ok(observability) => observability,
        Err(error) => {
            eprintln!("prnsd observability initialization failed: {error}");
            process::exit(1);
        }
    };
    if cli.log_format == cli::LogFormat::Human && managed.is_none() {
        splash::print_daemon();
    }
    tracing::info!(
        event = "daemon_starting",
        version = env!("CARGO_PKG_VERSION"),
    );
    match configuration::refresh_staged_bundled_source(&config_dir) {
        Ok(configuration::BundledSourceRefresh::Updated(staged)) => {
            tracing::info!(
                event = "nnpages_source_archive_updated",
                path = %staged.archive_path.display(),
                archive_bytes = staged.archive_bytes,
            );
            if let Err(error) = configuration::refresh_source_page(&config_dir) {
                tracing::warn!(
                    event = "nnpages_source_page_refresh_failed",
                    cause = "source_archive_update",
                    error = %error,
                );
            }
        }
        Ok(configuration::BundledSourceRefresh::Current(staged)) => {
            tracing::debug!(
                event = "nnpages_source_archive_current",
                path = %staged.archive_path.display(),
                archive_bytes = staged.archive_bytes,
            );
        }
        Ok(configuration::BundledSourceRefresh::OperatorOwned) => {
            tracing::debug!(
                event = "nnpages_source_archive_operator_owned",
                "leaving the hosted source archive untouched"
            );
        }
        Ok(
            configuration::BundledSourceRefresh::NotStaged
            | configuration::BundledSourceRefresh::BundleUnavailable,
        ) => {}
        Err(error) => {
            tracing::warn!(
                event = "nnpages_source_archive_update_failed",
                error = %error,
            );
        }
    }
    if let Some(path) = &config_path {
        tracing::info!(event = "config_loaded", path = %path.display());
    } else {
        tracing::info!(
            event = "config_defaulted",
            directory = %config_dir.display(),
        );
    }
    for diagnostic in config_warnings {
        tracing::warn!(
            event = "config_warning",
            code = diagnostic.code().as_str(),
            source = diagnostic.source(),
            line = diagnostic.line(),
            path = diagnostic.path(),
            diagnostic = %diagnostic,
        );
    }
    let nnpages = match nnpages::NnPagesCatalog::discover(&config_dir) {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(
                event = "nnpages_unavailable",
                root = %nnpages::root(&config_dir).display(),
                error = %error,
            );
            nnpages::NnPagesCatalog::empty(&config_dir)
        }
    };
    if let Err(error) = nnpages::recover_control_state(&config_dir) {
        tracing::warn!(event = "nnpages_control_recovery_failed", error = %error);
    }
    let network_identity =
        match identity::load_or_seed_network_identity(plan.network_identity_path.as_deref()) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::error!(event = "network_identity_failed", error = %error);
                observability.shutdown().await;
                process::exit(1);
            }
        };

    let storage_dir = config_dir.join("storage");
    let remote_control_identity_bootstrap =
        match identity::load_or_create_remote_control_identities(&storage_dir) {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                tracing::error!(event = "remote_control_identity_unavailable", error = %error);
                observability.shutdown().await;
                return Err(DaemonRunError::RemoteControlIdentityUnavailable(error));
            }
        };
    let (remote_control_identity_secrets, _) = remote_control_identity_bootstrap.into_parts();
    let persistent_secret = match identity::load_or_create_transport_identity(&storage_dir) {
        Ok(secret) => secret,
        Err(error) => match cli.persistence_policy {
            cli::PersistencePolicy::BestEffort => {
                tracing::error!(event = "identity_ephemeral", error = %error);
                personal_rns::runtime::generate_identity_secret()
            }
            cli::PersistencePolicy::Required => {
                tracing::error!(event = "transport_identity_unavailable", error = %error);
                observability.shutdown().await;
                return Err(DaemonRunError::TransportIdentityUnavailable(error));
            }
        },
    };
    let ble_identity =
        match personal_rns::runtime::load_or_create_ble_identity(&storage_dir.join("ble_identity"))
        {
            Ok(identity) => Some(identity),
            Err(error) => {
                tracing::error!(event = "ble_identity_failed", error = %error);
                None
            }
        };
    let browser_rendezvous_id = match personal_rns::runtime::load_or_create_browser_rendezvous_id(
        &storage_dir.join("browser_rendezvous_id"),
    ) {
        Ok(identity) => Some(identity),
        Err(error) => {
            tracing::error!(event = "browser_rendezvous_identity_failed", error = %error);
            None
        }
    };
    let mut shared_instance_credentials =
        SharedInstanceCredentials::from_identity_secret(&persistent_secret);
    if let SharedInstance::Enabled {
        rpc_key: Some(rpc_key),
        ..
    } = &plan.shared_instance
    {
        shared_instance_credentials = shared_instance_credentials.with_rpc_key(rpc_key.clone());
    }
    let blackhole_files = RnsBlackholeFiles::new(storage_dir.join("blackhole"));
    let routing_enabled = plan.transport.routing_enabled();
    let visible_secret = match plan.transport.identity_policy() {
        TransportIdentityPolicy::Persistent => persistent_secret.clone(),
        TransportIdentityPolicy::Ephemeral => personal_rns::runtime::generate_identity_secret(),
    };
    let visible_identity_hash =
        InMemoryNodeIdentity::from_secret_key_bytes(&visible_secret).identity_hash();
    let self_announcement_destination =
        services::node_page_destination_hash(&visible_identity_hash)
            .map_err(DaemonRunError::RemoteControlSelfAnnouncementUnavailable)?;
    let remote_control = RemoteControlService::new(
        remote_control_identity_secrets,
        RemoteControlPublicAppData::empty(),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Destination(self_announcement_destination),
    );
    let network_identity_hash = network_identity
        .as_ref()
        .map(|identity| InMemoryNodeIdentity::from_secret_key_bytes(identity).identity_hash());
    let mut interface_runtime =
        PlanRuntimeContext::with_rns_i2p_storage(storage_dir.clone(), visible_identity_hash);
    if let Some(identity) = ble_identity {
        interface_runtime = interface_runtime.with_ble_identity(identity);
    }
    let transport_secret = routing_enabled.then(|| visible_secret.clone());
    let non_routing_identity_secret = (!routing_enabled).then(|| visible_secret.clone());
    let protocol_policy = EngineProtocolPolicy {
        proof_form: if plan.protocol.use_implicit_proof {
            ProofForm::Implicit
        } else {
            ProofForm::Explicit
        },
        link_mtu_discovery: if plan.protocol.link_mtu_discovery {
            LinkMtuDiscovery::Enabled
        } else {
            LinkMtuDiscovery::Disabled
        },
        local_hop_count_override: if plan.protocol.randomize_local_hop_count {
            let entropy = personal_rns::runtime::generate_identity_secret();
            LocalHopCountOverride::from_entropy(entropy[0])
        } else {
            LocalHopCountOverride::Disabled
        },
        recursive_path_request_default: RecursivePathRequestDefault::Disabled,
    };

    let node_persistence = match NodePersistence::in_reticulum_dir(&config_dir) {
        Ok(node_persistence) => Some(node_persistence),
        Err(error) => {
            tracing::error!(event = "persistence_unavailable", %error);
            match cli.persistence_policy {
                cli::PersistencePolicy::BestEffort => None,
                cli::PersistencePolicy::Required => {
                    observability.shutdown().await;
                    return Err(DaemonRunError::PersistenceUnavailable(error));
                }
            }
        }
    };
    let timeline_origin = node_persistence
        .as_ref()
        .map(NodePersistence::timeline_origin)
        .unwrap_or_else(wall_clock_timeline_origin);
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
    let prepared_discovery = interface_discovery::PreparedDiscovery::from_plan(
        &plan,
        network_identity.clone(),
        &config_dir,
    );
    let (discovery_destination, prepared_discovery_publisher) =
        interface_discovery::publication::prepare(
            &plan,
            &visible_secret,
            network_identity.as_ref(),
        );
    let remote_management_transport =
        routing_enabled.then_some(services::TransportStatusIdentity {
            transport: visible_identity_hash,
            network: network_identity_hash,
            probe_responder: plan.probe_responder.is_enabled().then(|| {
                derive_single_destination_hash(
                    &visible_identity_hash,
                    "rnstransport",
                    &["probe"],
                )
                .expect("rnstransport.probe is a valid destination name")
            }),
        });
    let request_nnpages = nnpages.clone();
    let mut prns = PrnsNode::new_with_handle(move |handle| PrnsNodeRecipe {
        transport_identity: transport_secret,
        remote_control,
        pre_configured_destinations: std::iter::empty(),
        app_state: services::DaemonRequestState::new(
            handle,
            remote_management_transport,
            started,
            request_nnpages,
        ),
        storage: GrowableHeap,
        request_endpoints: services::DaemonRequestRoutes,
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state: &services::DaemonRequestState| {
            if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) = event {
                let _ = rotated_tx.send(destination);
            }
        },
    })
    .with_timeline_origin(timeline_origin)
    .with_crypto_pool(CryptoPoolConfig::Pooled {
        workers: PoolWorkers::Auto,
    })
    .with_resource_memory_limits(plan.resource_memory_limits)
    .with_protocol_policy(protocol_policy);
    tracing::info!(
        event = "resource_memory_limits_configured",
        incoming_bytes = plan.resource_memory_limits.incoming_bytes,
        outgoing_bytes = plan.resource_memory_limits.outgoing_bytes,
        changes_require_restart = true,
    );
    if let Err(error) = prns.register_preconfigured_destination(discovery_destination) {
        tracing::error!(
            event = "interface_discovery_destination_failed",
            error = ?error,
        );
        observability.shutdown().await;
        process::exit(1);
    }
    if let Some(secret) = non_routing_identity_secret {
        prns = match prns.with_non_routing_identity(secret) {
            Ok(prns) => prns,
            Err(error) => {
                tracing::error!(event = "non_routing_identity_failed", error = ?error);
                observability.shutdown().await;
                process::exit(1);
            }
        };
    }
    let prns_handle = prns.handle();
    if routing_enabled {
        if let Some(id) = browser_rendezvous_id {
            prns_handle.supervise(BrowserRendezvous::new(
                id,
                AutoWifiDevicePolicy::default(),
                personal_rns::interfaces::websocket::configured_policy(Default::default()),
            ));
        }
    }

    let interface_ownership = match interface_ownership::establish(
        &prns_handle,
        &plan,
        &interface_runtime,
        &shared_instance_credentials,
        visible_identity_hash,
        network_identity_hash,
        &blackhole_files,
    )
    .await
    {
        Ok(ownership) => ownership,
        Err(error) => {
            interface_ownership::report_join_error(&error);
            observability.shutdown().await;
            process::exit(1);
        }
    };
    let startup = interface_ownership.startup();

    let management_destinations = match interface_ownership.routing_tables() {
        Some(_) => match services::activate(&mut prns, &plan, &visible_secret, &nnpages) {
            Ok(destinations) => destinations,
            Err(_) => {
                observability.shutdown().await;
                process::exit(1);
            }
        },
        None => services::ManagementDestinations::none(),
    };
    let node_page_destination = management_destinations.node_page_destination();

    if plan.panic_on_interface_error && startup.failed != 0 {
        tracing::error!(
            event = "interface_failure_shutdown",
            failed = startup.failed,
        );
        observability.shutdown().await;
        process::exit(1);
    }

    let mut persistence = None;
    if interface_ownership.routing_tables().is_some() {
        if let Some(node_persistence) = node_persistence {
            persistence::restore(
                &mut prns,
                persistence::RestoreInputs {
                    store: node_persistence.store(),
                    vault: node_persistence.vault(),
                    blackhole_files: &blackhole_files,
                    blackhole_exchange: &plan.blackhole_exchange,
                    local_identity: visible_identity_hash,
                    timeline_origin,
                    progress: observability.state_restore_progress(),
                },
            );
            persistence = Some(persistence::prepare_worker(
                node_persistence,
                prns_handle.clone(),
                rotated_rx,
            ));
        }
    }

    let (prns, mut background_tasks) = background::start(background::BackgroundInputs {
        node: prns,
        handle: &prns_handle,
        plan: &plan,
        interface_runtime: &interface_runtime,
        ownership: interface_ownership,
        prepared_discovery,
        prepared_discovery_publisher: Some(prepared_discovery_publisher),
        network_identity: network_identity.clone(),
        config_dir: config_dir.clone(),
        blackhole_files,
        management_destinations,
        observability: &observability,
        started,
    });

    if let Some(managed) = managed.as_ref() {
        if let Err(error) = managed.publish_config_dir(&config_dir) {
            tracing::error!(event = "managed_config_publish_failed", error = %error);
            observability.shutdown().await;
            process::exit(1);
        }
    }
    tracing::info!(
        event = if startup.degraded() {
            "daemon_ready_degraded"
        } else {
            "daemon_ready"
        },
        transport = routing_enabled,
        online = startup.online,
        listening = startup.listening,
        retrying = startup.retrying,
        failed = startup.failed,
    );
    if let Some(managed) = managed.as_ref() {
        if let Err(error) = managed.mark_ready() {
            tracing::error!(event = "managed_ready_failed", error = %error);
            observability.shutdown().await;
            process::exit(1);
        }
    }
    let ready_status = daemon_status(&prns_handle);
    if let Some(ready) = ready {
        let _ = ready.send(DaemonReady {
            config_dir: config_dir.clone(),
            managed_state_dir: managed
                .as_ref()
                .map(|process| process.state_dir().to_path_buf()),
            status: ready_status,
        });
    }
    #[cfg(all(feature = "tray", target_os = "linux"))]
    let (_tray, shutdown) = match shutdown {
        Some(shutdown) => (None, Some(shutdown)),
        None => match crate::tray::start(
            config_dir.clone(),
            managed
                .as_ref()
                .map(|process| process.state_dir().to_path_buf()),
            ready_status,
        )
        .await
        {
            Ok((tray, tray_shutdown, publisher)) => {
                tracing::info!(event = "tray_started");
                status_publisher = Some(publisher);
                (Some(tray), Some(tray_shutdown))
            }
            Err(crate::tray::TrayStartError::Platform(error)) => {
                tracing::info!(event = "tray_unavailable", cause = "platform", error = %error);
                (None, None)
            }
            Err(crate::tray::TrayStartError::Actions(error)) => {
                tracing::warn!(event = "tray_unavailable", cause = "actions", error = %error);
                (None, None)
            }
        },
    };
    let tray_status_task = status_publisher
        .map(|publisher| tokio::spawn(publish_daemon_status(prns_handle.clone(), publisher)));
    let mut interface_failure = None;
    let mut terminal_error = None;
    let mut active_plan = plan.clone();
    let active_config_path = config_path.unwrap_or_else(|| config_dir.join("config"));
    let mut node_run = Box::pin(prns.run());
    let mut persistence_run = Box::pin(persistence::run_until_shutdown(
        persistence,
        managed.as_ref(),
        shutdown,
        operating_system_shutdown,
    ));
    let mut nnpages_refresh_tick = Box::pin(tokio::time::sleep(NNPAGES_REFRESH_INTERVAL));
    let mut nnpages_refresh_tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            result = &mut node_run => {
                match result {
                    Ok(()) => {
                        tracing::error!(event = "node_stopped");
                        terminal_error = Some(DaemonRunError::NodeStopped);
                    }
                    Err(error) => {
                        tracing::error!(event = "node_panic_shutdown", error = ?error);
                        terminal_error = Some(DaemonRunError::NodePanicked(error));
                    }
                }
                break;
            }
            status = &mut persistence_run => {
                terminal_error = match status {
                    PersistenceFlushStatus::Landed => None,
                    PersistenceFlushStatus::NodeStopped => {
                        Some(DaemonRunError::PersistenceWorkerStopped)
                    }
                    PersistenceFlushStatus::Failed
                        if cli.persistence_policy == cli::PersistencePolicy::Required =>
                    {
                        Some(DaemonRunError::PersistenceFlushFailed)
                    }
                    PersistenceFlushStatus::Failed => None,
                };
                break;
            },
            failed = interface_failure::wait(
                &prns_handle,
                background_tasks.interface_failure_watch(),
                active_plan.panic_on_interface_error,
            ) => {
                interface_failure = Some(failed);
                tracing::error!(
                    event = "interface_failure_shutdown",
                    interface = ?failed,
                );
                break;
            }
            request = next_reload(managed.as_ref()) => {
                match request {
                    Ok(request) => {
                        let (result, replacement) = apply_reload(
                            &request,
                            &active_config_path,
                            &active_plan,
                            &mut background_tasks,
                            &prns_handle,
                        ).await;
                        if let Some(replacement) = replacement {
                            active_plan = replacement;
                        }
                        if let Some(managed) = managed.as_ref() {
                            if let Err(error) = managed.finish_reload(&request, result) {
                                tracing::error!(event = "interface_apply_result_failed", error = %error);
                            }
                        }
                    }
                    Err(error) => {
                        tracing::error!(event = "interface_apply_request_failed", error = %error);
                    }
                }
            }
            _ = &mut nnpages_refresh_tick => {
                if let Some(destination) = node_page_destination {
                    let nnpages = nnpages.clone();
                    let handle = prns_handle.clone();
                    nnpages_refresh_tasks.spawn(async move {
                        match nnpages.refresh(&handle, destination).await {
                            Ok(report) if report.added != 0 || report.removed != 0 => {
                                tracing::info!(
                                    event = "nnpages_refreshed",
                                    discovered = report.discovered,
                                    added = report.added,
                                    removed = report.removed,
                                    unchanged = report.unchanged,
                                    settings = report.settings_status.as_control_value(),
                                    settings_changed = report.settings_changed,
                                    cause = "periodic",
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::warn!(
                                    event = "nnpages_refresh_failed",
                                    cause = "periodic",
                                    error = %error,
                                );
                            }
                        }
                    });
                }
                nnpages_refresh_tick
                    .as_mut()
                    .reset(tokio::time::Instant::now() + NNPAGES_REFRESH_INTERVAL);
            }
            request = nnpages::next_control_request(&config_dir) => {
                match request {
                    Ok(request) => {
                        let nnpages = nnpages.clone();
                        let handle = prns_handle.clone();
                        nnpages_refresh_tasks.spawn(async move {
                            match request.kind() {
                                nnpages::NnPagesControlKind::Refresh => {
                                    let result = match node_page_destination {
                                        Some(destination) => {
                                            nnpages.refresh(&handle, destination).await
                                        }
                                        None => Err(
                                            nnpages::NnPagesRefreshError::DestinationUnavailable,
                                        ),
                                    };
                                    match &result {
                                        Ok(report) => tracing::info!(
                                            event = "nnpages_refreshed",
                                            discovered = report.discovered,
                                            added = report.added,
                                            removed = report.removed,
                                            unchanged = report.unchanged,
                                            settings = report.settings_status.as_control_value(),
                                            settings_changed = report.settings_changed,
                                            cause = "operator",
                                        ),
                                        Err(error) => tracing::warn!(
                                            event = "nnpages_refresh_failed",
                                            cause = "operator",
                                            error = %error,
                                        ),
                                    }
                                    if let Err(error) = request.finish(result) {
                                        tracing::warn!(
                                            event = "nnpages_refresh_result_failed",
                                            error = %error,
                                        );
                                    }
                                }
                                nnpages::NnPagesControlKind::Announce => {
                                    let result = match node_page_destination {
                                        Some(destination)
                                            if nnpages::is_page_available(&nnpages.index_path()) =>
                                        {
                                            match handle
                                                .announce_now(services::announce_for(
                                                    destination,
                                                    Some(&nnpages.node_name_path()),
                                                ))
                                                .await
                                            {
                                                Ok(_) => Ok(()),
                                                Err(error) => {
                                                    tracing::warn!(
                                                        event = "nnpages_announce_failed",
                                                        cause = "operator",
                                                        error = ?error,
                                                    );
                                                    Err(nnpages::NnPagesControlFailure::AnnounceSend)
                                                }
                                            }
                                        }
                                        Some(_) => {
                                            tracing::warn!(
                                                event = "nnpages_announce_failed",
                                                cause = "index_unavailable",
                                            );
                                            Err(nnpages::NnPagesControlFailure::IndexUnavailable)
                                        }
                                        None => {
                                            tracing::warn!(
                                                event = "nnpages_announce_failed",
                                                cause = "destination_unavailable",
                                            );
                                            Err(nnpages::NnPagesControlFailure::DestinationUnavailable)
                                        }
                                    };
                                    if result.is_ok() {
                                        tracing::info!(
                                            event = "nnpages_announced",
                                            cause = "operator",
                                        );
                                    }
                                    if let Err(error) = request.finish_announce(result) {
                                        tracing::warn!(
                                            event = "nnpages_announce_result_failed",
                                            error = %error,
                                        );
                                    }
                                }
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(
                            event = "nnpages_refresh_request_failed",
                            error = %error,
                        );
                    }
                }
            }
            completed = nnpages_refresh_tasks.join_next(), if !nnpages_refresh_tasks.is_empty() => {
                if let Some(Err(error)) = completed {
                    tracing::warn!(
                        event = "nnpages_refresh_task_failed",
                        error = %error,
                    );
                }
            }
        }
    }
    nnpages_refresh_tasks.shutdown().await;
    drop(persistence_run);
    drop(node_run);
    if let Some(task) = tray_status_task {
        task.abort();
        let _ = task.await;
    }
    background_tasks.shutdown().await;
    observability.shutdown().await;
    if let Some(managed) = managed {
        managed.hold_runtime_lock_until_process_exit();
    }
    if interface_failure.is_some() {
        return Err(DaemonRunError::InterfaceFailed);
    }
    match terminal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn daemon_status(handle: &personal_rns::runtime::PrnsNodeHandle) -> DaemonStatus {
    let inventory = logical_interface_inventory(handle.interface_inventory());
    daemon_status_from_connections(
        inventory
            .into_iter()
            .map(|interface| interface.snapshot.connection),
    )
}

fn daemon_status_from_connections(
    connections: impl IntoIterator<Item = ConnectionState>,
) -> DaemonStatus {
    let mut status = DaemonStatus::default();
    for connection in connections {
        status.interface_count = status.interface_count.saturating_add(1);
        match connection {
            ConnectionState::Initializing | ConnectionState::Reconnecting => {
                status.retrying = status.retrying.saturating_add(1);
            }
            ConnectionState::Degraded => {
                status.impaired = status.impaired.saturating_add(1);
            }
            ConnectionState::Failed | ConnectionState::Disconnected => {
                status.unavailable = status.unavailable.saturating_add(1);
            }
            ConnectionState::Connected | ConnectionState::Disabled | ConnectionState::Unknown => {}
        }
    }
    status
}

async fn publish_daemon_status(
    handle: personal_rns::runtime::PrnsNodeHandle,
    publisher: DaemonStatusPublisher,
) {
    let mut interval = tokio::time::interval(TRAY_STATUS_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        publisher.publish(daemon_status(&handle));
    }
}

async fn next_reload(managed: Option<&ManagedProcess>) -> Result<ReloadRequest, ServiceError> {
    let Some(managed) = managed else {
        return future::pending().await;
    };
    loop {
        if let Some(request) = managed.reload_request()? {
            return Ok(request);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn apply_reload(
    request: &ReloadRequest,
    config_path: &Path,
    active_plan: &personal_rns::config::DaemonPlan,
    background: &mut background::BackgroundTasks,
    handle: &personal_rns::runtime::PrnsNodeHandle,
) -> (ReloadResult, Option<personal_rns::config::DaemonPlan>) {
    let bytes = match std::fs::read(config_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(
                event = "interface_apply_rejected",
                reason = "config_read_failed",
                error_kind = ?error.kind(),
            );
            return (ReloadResult::Rejected, None);
        }
    };
    if config_digest(&bytes) != request.digest() {
        tracing::warn!(
            event = "interface_apply_rejected",
            reason = "digest_mismatch"
        );
        return (ReloadResult::Rejected, None);
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            tracing::warn!(event = "interface_apply_rejected", reason = "invalid_utf8");
            return (ReloadResult::Rejected, None);
        }
    };
    let replacement = match personal_rns::config::parse_and_plan_named(
        config_path.display().to_string(),
        &text,
    ) {
        Ok(report) => report.value,
        Err(errors) => {
            tracing::warn!(
                event = "interface_apply_rejected",
                reason = "invalid_configuration",
                diagnostics = errors.len(),
            );
            return (ReloadResult::Rejected, None);
        }
    };
    if non_interface_configuration_changed(active_plan, &replacement) {
        tracing::info!(event = "interface_apply_restart_required");
        return (ReloadResult::RestartRequired, None);
    }
    let result = background
        .apply_interfaces(handle, replacement.clone())
        .await;
    let applied =
        matches!(result, ReloadResult::Applied | ReloadResult::Unchanged).then_some(replacement);
    (result, applied)
}

fn non_interface_configuration_changed(
    active: &personal_rns::config::DaemonPlan,
    replacement: &personal_rns::config::DaemonPlan,
) -> bool {
    let mut active = active.clone();
    active.interfaces.clear();
    let mut replacement = replacement.clone();
    replacement.interfaces.clear();
    active != replacement
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn tray_health_folds_logical_connection_states_without_penalizing_disabled_slots() {
        let status = daemon_status_from_connections([
            ConnectionState::Connected,
            ConnectionState::Initializing,
            ConnectionState::Reconnecting,
            ConnectionState::Degraded,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
            ConnectionState::Disabled,
            ConnectionState::Unknown,
        ]);

        assert_eq!(
            status,
            DaemonStatus {
                interface_count: 8,
                retrying: 2,
                impaired: 1,
                unavailable: 2,
            }
        );
    }

    #[test]
    fn resource_memory_limit_changes_require_a_daemon_restart() {
        let active = personal_rns::config::parse_and_plan(
            "[prns]\nresource_mem_in = 64 MiB\nresource_mem_out = 64 MiB\n",
        )
        .unwrap()
        .value;
        let changed = personal_rns::config::parse_and_plan(
            "[prns]\nresource_mem_in = 32 MiB\nresource_mem_out = 64 MiB\n",
        )
        .unwrap()
        .value;

        assert!(non_interface_configuration_changed(&active, &changed));
    }
}
