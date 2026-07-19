mod interface_failure;

use std::process;

use crate::{
    blackhole_exchange, cli, construct, identity, interface_discovery, management_announces,
    observability, persist, probe_responder, remote_management, request_services, splash,
};
use personal_rns::config::{
    discover, parse_and_plan_named, ConfiguredInterfaceLifecycle, SharedInstance,
    SharedInstanceTransport as ConfigSharedInstanceTransport, TransportIdentityPolicy,
};
use personal_rns::engine::{
    EngineProtocolPolicy, LinkMtuDiscovery, LocalHopCountOverride, ProofForm,
};
use personal_rns::from_plan::PlanAttachments;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::FileVault;
use personal_rns::identity::IdentitySigner;
use personal_rns::persistence::FileStore;
use personal_rns::routes;
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, Manual, PrnsEvent, PrnsNode, PrnsNodeRecipe,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, RnsBlackholeFiles, Role,
    SharedInstanceCredentials, SharedInstanceIntent,
    SharedInstanceTransport as RuntimeSharedInstanceTransport,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::PlanRuntimeContext;
use prnsd_control::ManagedProcess;

const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n";

pub(super) async fn run(cli: cli::DaemonArgs, managed: Option<ManagedProcess>) {
    let started = std::time::Instant::now();
    let discovered_config = match discover(cli.config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("prnsd: config discovery failed: {error}");
            process::exit(1);
        }
    };
    let (config_text, config_source) = match &discovered_config.config {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => (text, path.display().to_string()),
            Err(error) => {
                eprintln!("prnsd: could not read config {}: {error}", path.display());
                process::exit(1);
            }
        },
        None => (DEFAULT_CONFIG.to_string(), "<built-in config>".to_string()),
    };

    let report = match parse_and_plan_named(&config_source, &config_text) {
        Ok(report) => report,
        Err(errors) => {
            for diagnostic in errors.diagnostics() {
                eprintln!("{diagnostic}");
            }
            process::exit(1);
        }
    };
    let plan = report.value;
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
    if let Some(path) = &discovered_config.config {
        tracing::info!(event = "config_loaded", path = %path.display());
    } else {
        tracing::info!(
            event = "config_defaulted",
            directory = %discovered_config.dir.display(),
        );
    }
    for diagnostic in report.warnings {
        tracing::warn!(
            event = "config_warning",
            code = diagnostic.code().as_str(),
            source = diagnostic.source(),
            line = diagnostic.line(),
            path = diagnostic.path(),
            diagnostic = %diagnostic,
        );
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

    let storage_dir = discovered_config.dir.join("storage");
    let persistent_secret = identity::load_or_seed_transport_identity(&storage_dir);
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
    let network_identity_hash = network_identity
        .as_ref()
        .map(|identity| InMemoryNodeIdentity::from_secret_key_bytes(identity).identity_hash());
    let interface_runtime =
        PlanRuntimeContext::with_rns_i2p_storage(storage_dir.clone(), visible_identity_hash);
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
    };

    let persist_dir = persist::store_dir(&storage_dir);
    let store = FileStore::new(&persist_dir);
    let timeline_origin = boot_timeline_origin(&store);
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut prepared_discovery = interface_discovery::PreparedDiscovery::from_plan(
        &plan,
        network_identity.clone(),
        &discovered_config.dir,
    );
    let (discovery_destination, prepared_discovery_publisher) =
        interface_discovery::publication::prepare(
            &plan,
            &visible_secret,
            network_identity.as_ref(),
        )
        .unzip();
    let remote_management_transport =
        routing_enabled.then_some(request_services::TransportStatusIdentity {
            transport: visible_identity_hash,
            network: network_identity_hash,
        });
    let mut prns = PrnsNode::new_with_handle(move |handle| PrnsNodeRecipe {
        transport_identity: transport_secret,
        pre_configured_destinations: std::iter::empty(),
        app_state: request_services::DaemonRequestState::new(
            handle,
            remote_management_transport,
            started,
        ),
        storage: GrowableHeap,
        routes: routes![
            remote_management::StatusRoute,
            remote_management::PathRoute,
            blackhole_exchange::ListRoute
        ],
        interfaces: Manual,
        on_event: move |event, _state: &request_services::DaemonRequestState| {
            if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) = event {
                let _ = rotated_tx.send(destination);
            }
        },
    })
    .with_timeline_origin(timeline_origin)
    .with_protocol_policy(protocol_policy);
    if let Some(destination) = discovery_destination {
        if let Err(error) = prns.register_preconfigured_destination(destination) {
            tracing::error!(
                event = "interface_discovery_destination_failed",
                error = ?error,
            );
            observability.shutdown().await;
            process::exit(1);
        }
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

    let mut owns_tables = false;
    let mut constructed_interfaces = Vec::new();
    let mut bootstrap_attachments = PlanAttachments::default();
    let mut startup = construct::StartupInterfaceReport::default();
    match &plan.shared_instance {
        SharedInstance::Enabled {
            name,
            transport,
            instance_port,
            control_port,
            forced_bitrate,
            ..
        } => {
            let ports = InstancePorts {
                bus: *instance_port,
                control: *control_port,
            };
            let runtime_transport = match transport {
                ConfigSharedInstanceTransport::Tcp => RuntimeSharedInstanceTransport::Tcp,
                ConfigSharedInstanceTransport::Unix => {
                    #[cfg(target_os = "linux")]
                    {
                        RuntimeSharedInstanceTransport::AbstractUnix {
                            socket_path: name.clone(),
                        }
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        tracing::warn!(
                            event = "shared_instance_unix_fallback",
                            configured_name = %name,
                            fallback = "tcp",
                        );
                        RuntimeSharedInstanceTransport::Tcp
                    }
                }
            };
            let shared_policy = personal_rns::interfaces::shared_instance::core::configured_policy(
                personal_rns::interfaces::ConfiguredInterfacePolicy {
                    bitrate: *forced_bitrate,
                    ..Default::default()
                },
            );
            match join_shared_instance(
                &prns_handle,
                SharedInstanceIntent {
                    credentials: shared_instance_credentials.clone(),
                    blackhole_source: visible_identity_hash,
                    blackhole_files: blackhole_files.clone(),
                    ports,
                    transport: runtime_transport,
                    policy: shared_policy,
                    on_existing: OnExisting::JoinAsClient,
                },
            )
            .await
            {
                Ok(Role::BecameInstance) => {
                    tracing::info!(
                        event = "shared_instance_started",
                        bus_port = ports.bus,
                        control_port = ports.control,
                        instance_name = %name,
                    );
                    startup.listening = startup.listening.saturating_add(1);
                    let constructed =
                        construct::construct_interfaces(&prns_handle, &plan, &interface_runtime)
                            .await;
                    startup.merge(constructed.startup);
                    bootstrap_attachments = constructed
                        .runtime
                        .for_lifecycle(ConfiguredInterfaceLifecycle::BootstrapOnly);
                    constructed_interfaces = constructed.attached;
                    owns_tables = true;
                }
                Ok(Role::JoinedAsClient { of }) => {
                    startup.online = startup.online.saturating_add(1);
                    tracing::info!(event = "shared_instance_joined");
                    tracing::debug!(event = "shared_instance_joined_detail", instance = %of);
                }
                Err(JoinError::InstanceAlreadyRunning { at }) => {
                    tracing::error!(event = "shared_instance_refused", endpoint = %at);
                    observability.shutdown().await;
                    process::exit(1);
                }
                Err(JoinError::EndpointUnavailable { endpoint, kind }) => {
                    tracing::error!(
                        event = "shared_instance_endpoint_unavailable",
                        endpoint = endpoint.as_str(),
                        error_kind = ?kind,
                    );
                    observability.shutdown().await;
                    process::exit(1);
                }
            }
        }
        SharedInstance::Disabled => {
            tracing::info!(event = "standalone_node_started");
            let constructed =
                construct::construct_interfaces(&prns_handle, &plan, &interface_runtime).await;
            startup.merge(constructed.startup);
            bootstrap_attachments = constructed
                .runtime
                .for_lifecycle(ConfiguredInterfaceLifecycle::BootstrapOnly);
            constructed_interfaces = constructed.attached;
            owns_tables = true;
        }
    }

    let mut management_destinations = Vec::new();
    if owns_tables {
        if let Some(allowed) = plan.remote_management.allowed() {
            match remote_management::activate(&mut prns, visible_secret.clone(), allowed) {
                Ok(destination) => {
                    management_destinations.push(destination);
                    tracing::info!(
                        event = "remote_management_enabled",
                        destination = ?destination.as_bytes(),
                        allowed_identities = allowed.len(),
                    );
                }
                Err(error) => {
                    tracing::error!(
                        event = "remote_management_start_failed",
                        error = ?error,
                    );
                    observability.shutdown().await;
                    process::exit(1);
                }
            }
        }
        if plan.probe_responder.is_enabled() {
            match probe_responder::activate(&mut prns, visible_secret.clone()) {
                Ok(destination) => {
                    management_destinations.push(destination);
                    tracing::info!(
                        event = "probe_responder_enabled",
                        destination = ?destination.as_bytes(),
                    );
                }
                Err(error) => {
                    tracing::error!(
                        event = "probe_responder_start_failed",
                        error = ?error,
                    );
                    observability.shutdown().await;
                    process::exit(1);
                }
            }
        }
        if plan.blackhole_exchange.publication().is_enabled() {
            match blackhole_exchange::activate(&mut prns, visible_secret.clone()) {
                Ok(destination) => {
                    management_destinations.push(destination);
                    tracing::info!(
                        event = "blackhole_publisher_enabled",
                        destination = ?destination.as_bytes(),
                    );
                }
                Err(error) => {
                    tracing::error!(
                        event = "blackhole_publisher_start_failed",
                        error = ?error,
                    );
                    observability.shutdown().await;
                    process::exit(1);
                }
            }
        }
    }

    if plan.panic_on_interface_error && startup.failed != 0 {
        tracing::error!(
            event = "interface_failure_shutdown",
            failed = startup.failed,
        );
        observability.shutdown().await;
        process::exit(1);
    }

    let mut persistence = None;
    if owns_tables {
        let mut restore_progress = observability.state_restore_progress();
        let vault = FileVault::new(&persist_dir);
        let mut restored_blackholes =
            match blackhole_files.load_local(visible_identity_hash, timeline_origin) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::warn!(event = "blackhole_restore_failed", error = %error);
                    Vec::new()
                }
            };
        for source in plan.blackhole_exchange.sources() {
            match blackhole_files.load_source(*source, timeline_origin) {
                Ok(entries) => restored_blackholes.extend(entries),
                Err(error) => tracing::warn!(
                    event = "blackhole_source_restore_failed",
                    source = ?source.as_bytes(),
                    error = %error,
                ),
            }
        }
        let blackholes = prns.seed_blackholed_identities(restored_blackholes);
        let routes = match restore_progress.as_mut() {
            Some(progress) => prns.seed_routes_from_store_reporting(&store, |route_progress| {
                progress.observe(route_progress);
            }),
            None => prns.seed_routes_from_store(&store),
        };
        let destination_identities = prns.seed_destination_identities_from_store(&store);
        let tunnels = prns.seed_tunnels_from_store(&store);
        let ratchets = prns.seed_self_ratchets_from_vault(&vault);
        if let Some(progress) = restore_progress {
            progress.finish();
        }
        tracing::info!(
            event = "state_restored",
            blackholes = blackholes.seeded_count,
            routes = routes.seeded_count,
            destination_identities = destination_identities.seeded_count,
            tunnels = tunnels.seeded_count,
            ratchets = ratchets.seeded_count,
            refused = blackholes.refused_count
                + routes.refused_count
                + destination_identities.refused_count
                + tunnels.refused_count
                + ratchets.refused_count,
            dropped = blackholes.dropped_count
                + routes.dropped_count
                + destination_identities.dropped_count
                + tunnels.dropped_count
                + ratchets.dropped_count,
        );
        persistence = Some(persist::Persistence::new(
            prns_handle.clone(),
            store,
            vault,
            rotated_rx,
            persist::PERSIST_INTERVAL,
        ));
    }

    let monitored_interfaces = interface_discovery::MonitoredInterfaces::new(
        constructed_interfaces.iter().map(|interface| interface.id),
    );
    let interface_failure_watch = monitored_interfaces.subscribe();
    let bootstrap_interfaces = if owns_tables {
        interface_discovery::BootstrapInterfaces::prepare(
            &plan,
            interface_runtime.clone(),
            bootstrap_attachments,
            monitored_interfaces,
        )
    } else {
        None
    };
    let discovery_task = if owns_tables {
        match prepared_discovery.take() {
            Some(discovery) => {
                let observer = discovery.observer();
                prns = prns.with_accepted_announce_observer(move |observation| {
                    observer.observe(observation);
                });
                let clock = prns.clock();
                Some(discovery.spawn(prns_handle.clone(), clock, bootstrap_interfaces))
            }
            None => None,
        }
    } else {
        None
    };
    let discovery_publication_task = if owns_tables {
        match prepared_discovery_publisher {
            Some(publisher) => {
                let clock = prns.clock();
                match publisher.spawn(prns_handle.clone(), clock, constructed_interfaces) {
                    Ok(task) => task,
                    Err(error) => {
                        tracing::error!(
                            event = "interface_discovery_publisher_start_failed",
                            error = %error,
                        );
                        None
                    }
                }
            }
            None => None,
        }
    } else {
        None
    };
    let management_announce_task =
        management_announces::spawn(prns_handle.clone(), management_destinations);
    let blackhole_update_task = if owns_tables {
        blackhole_exchange::spawn_updater(
            prns_handle.clone(),
            prns.clock(),
            blackhole_files,
            &plan.blackhole_exchange,
        )
    } else {
        None
    };
    #[cfg(feature = "otlp")]
    let metrics_task = observability.metrics_reporter().map(|reporter| {
        let runtime_up = reporter.runtime_up_handle();
        (
            tokio::spawn(reporter.run(prns_handle.clone(), started)),
            runtime_up,
        )
    });

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
    let mut interface_failure = None;
    tokio::select! {
        () = prns.run() => {}
        () = persist::run_until_shutdown(persistence, managed.as_ref()) => {}
        failed = interface_failure::wait(
            &prns_handle,
            interface_failure_watch,
            plan.panic_on_interface_error,
        ) => {
            interface_failure = Some(failed);
            tracing::error!(
                event = "interface_failure_shutdown",
                interface = ?failed,
            );
        }
    }
    if let Some(discovery) = discovery_task {
        discovery.shutdown().await;
    }
    if let Some(publisher) = discovery_publication_task {
        if let Err(error) = publisher.shutdown().await {
            tracing::warn!(event = "interface_discovery_publisher_task_failed", error = %error);
        }
    }
    if let Some(task) = management_announce_task {
        task.shutdown().await;
    }
    if let Some(task) = blackhole_update_task {
        task.shutdown().await;
    }
    #[cfg(feature = "otlp")]
    if let Some((task, runtime_up)) = metrics_task {
        task.abort();
        let _ = task.await;
        runtime_up.record(0, &[]);
    }
    observability.shutdown().await;
    if let Some(managed) = managed {
        managed.hold_runtime_lock_until_process_exit();
    }
    if interface_failure.is_some() {
        process::exit(1);
    }
}
