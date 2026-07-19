mod configuration;
mod configured_interfaces;
mod identity;
mod interface_failure;
mod interface_ownership;

pub(crate) use configured_interfaces::{
    construct as construct_configured_interfaces, AttachedConfiguredInterface,
};

pub(crate) use configuration::DEFAULT_CONFIG;

use std::process;

use crate::{cli, interface_discovery, observability, persistence, services, splash};
use personal_rns::config::{SharedInstance, TransportIdentityPolicy};
use personal_rns::engine::{
    EngineProtocolPolicy, LinkMtuDiscovery, LocalHopCountOverride, ProofForm,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::FileVault;
use personal_rns::identity::IdentitySigner;
use personal_rns::persistence::FileStore;
use personal_rns::routes;
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, Manual, PrnsEvent, PrnsNode, PrnsNodeRecipe,
};
use personal_rns::shared_instance::{RnsBlackholeFiles, SharedInstanceCredentials};
use personal_rns::storage::GrowableHeap;
use personal_rns::PlanRuntimeContext;
use prnsd_control::ManagedProcess;

pub(super) async fn run(cli: cli::DaemonArgs, managed: Option<ManagedProcess>) {
    let started = std::time::Instant::now();
    let configuration::LoadedConfiguration {
        directory: config_dir,
        path: config_path,
        plan,
        warnings: config_warnings,
    } = configuration::load_or_exit(cli.config.as_deref());
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

    let persist_dir = persistence::store_dir(&storage_dir);
    let store = FileStore::new(&persist_dir);
    let timeline_origin = boot_timeline_origin(&store);
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut prepared_discovery = interface_discovery::PreparedDiscovery::from_plan(
        &plan,
        network_identity.clone(),
        &config_dir,
    );
    let (discovery_destination, prepared_discovery_publisher) =
        interface_discovery::publication::prepare(
            &plan,
            &visible_secret,
            network_identity.as_ref(),
        )
        .unzip();
    let remote_management_transport =
        routing_enabled.then_some(services::TransportStatusIdentity {
            transport: visible_identity_hash,
            network: network_identity_hash,
        });
    let mut prns = PrnsNode::new_with_handle(move |handle| PrnsNodeRecipe {
        transport_identity: transport_secret,
        pre_configured_destinations: std::iter::empty(),
        app_state: services::DaemonRequestState::new(handle, remote_management_transport, started),
        storage: GrowableHeap,
        routes: routes![
            services::StatusRoute,
            services::PathRoute,
            services::ListRoute
        ],
        interfaces: Manual,
        on_event: move |event, _state: &services::DaemonRequestState| {
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
        Some(_) => match services::activate(&mut prns, &plan, &visible_secret) {
            Ok(destinations) => destinations,
            Err(_) => {
                observability.shutdown().await;
                process::exit(1);
            }
        },
        None => services::ManagementDestinations::none(),
    };

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
        let vault = FileVault::new(&persist_dir);
        persistence::restore(
            &mut prns,
            persistence::RestoreInputs {
                store: &store,
                vault: &vault,
                blackhole_files: &blackhole_files,
                blackhole_exchange: &plan.blackhole_exchange,
                local_identity: visible_identity_hash,
                timeline_origin,
                progress: observability.state_restore_progress(),
            },
        );
        persistence = Some(persistence::prepare_worker(
            prns_handle.clone(),
            store,
            vault,
            rotated_rx,
        ));
    }

    let management_announce_task =
        services::spawn_management_announcements(prns_handle.clone(), management_destinations);
    let (
        interface_failure_watch,
        discovery_task,
        discovery_publication_task,
        blackhole_update_task,
    ) = match interface_ownership.into_routing_tables() {
        Some(interface_ownership::RoutingTableOwnership {
            configured_interfaces,
            bootstrap_attachments,
        }) => {
            let monitored_interfaces = interface_discovery::MonitoredInterfaces::new(
                configured_interfaces.iter().map(|interface| interface.id),
            );
            let interface_failure_watch = monitored_interfaces.subscribe();
            let bootstrap_interfaces = interface_discovery::BootstrapInterfaces::prepare(
                &plan,
                interface_runtime.clone(),
                bootstrap_attachments,
                monitored_interfaces,
            );
            let discovery_task = match prepared_discovery.take() {
                Some(discovery) => {
                    let observer = discovery.observer();
                    prns = prns.with_accepted_announce_observer(move |observation| {
                        observer.observe(observation);
                    });
                    let clock = prns.clock();
                    Some(discovery.spawn(prns_handle.clone(), clock, bootstrap_interfaces))
                }
                None => None,
            };
            let discovery_publication_task = match prepared_discovery_publisher {
                Some(publisher) => {
                    let clock = prns.clock();
                    match publisher.spawn(prns_handle.clone(), clock, configured_interfaces) {
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
            };
            let blackhole_update_task = services::spawn_blackhole_updater(
                prns_handle.clone(),
                prns.clock(),
                blackhole_files,
                &plan.blackhole_exchange,
            );
            (
                interface_failure_watch,
                discovery_task,
                discovery_publication_task,
                blackhole_update_task,
            )
        }
        None => {
            let monitored_interfaces =
                interface_discovery::MonitoredInterfaces::new(std::iter::empty());
            (monitored_interfaces.subscribe(), None, None, None)
        }
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
        () = persistence::run_until_shutdown(persistence, managed.as_ref()) => {}
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
