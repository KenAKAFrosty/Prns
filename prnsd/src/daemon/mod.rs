mod background;
mod configuration;
mod configured_interfaces;
mod identity;
mod interface_failure;
mod interface_ownership;

pub(crate) use configured_interfaces::{
    construct as construct_configured_interfaces, AttachedConfiguredInterface,
};

pub(crate) use configuration::DEFAULT_CONFIG;

use std::future;
use std::path::Path;
use std::process;
use std::time::Duration;

use crate::shutdown::ShutdownSignal;
use crate::{cli, interface_discovery, observability, persistence, services, splash};
use personal_rns::browser_rendezvous::BrowserRendezvous;
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
    boot_timeline_origin, CryptoPoolConfig, Diagnostic, Manual, PoolWorkers, PrnsEvent, PrnsNode,
    PrnsNodeRecipe,
};
use personal_rns::shared_instance::{RnsBlackholeFiles, SharedInstanceCredentials};
use personal_rns::storage::GrowableHeap;
use personal_rns::wifi_auto::AutoWifiDevicePolicy;
use personal_rns::PlanRuntimeContext;
use prnsd_control::{config_digest, ManagedProcess, ReloadRequest, ReloadResult, ServiceError};

pub(super) async fn run(
    cli: cli::DaemonArgs,
    managed: Option<ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
) {
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
    };

    let persist_dir = persistence::store_dir(&storage_dir);
    let store = FileStore::new(&persist_dir);
    let timeline_origin = boot_timeline_origin(&store);
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
    .with_crypto_pool(CryptoPoolConfig::Pooled {
        workers: PoolWorkers::Auto,
    })
    .with_protocol_policy(protocol_policy);
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
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    #[cfg(all(feature = "tray", target_os = "linux"))]
    let (_tray, shutdown) = match shutdown {
        Some(shutdown) => (None, Some(shutdown)),
        None => match crate::tray::start() {
            Ok((tray, shutdown)) => {
                tracing::info!(event = "tray_started");
                (Some(tray), Some(shutdown))
            }
            Err(error) => {
                tracing::warn!(event = "tray_unavailable", error = %error);
                (None, None)
            }
        },
    };
    let mut interface_failure = None;
    let mut node_failure = false;
    let mut active_plan = plan.clone();
    let active_config_path = config_path.unwrap_or_else(|| config_dir.join("config"));
    let mut node_run = Box::pin(prns.run());
    let mut persistence_run = Box::pin(persistence::run_until_shutdown(
        persistence,
        managed.as_ref(),
        shutdown,
    ));
    loop {
        tokio::select! {
            result = &mut node_run => {
                node_failure = true;
                match result {
                    Ok(()) => tracing::error!(event = "node_stopped"),
                    Err(error) => tracing::error!(event = "node_panic_shutdown", error = ?error),
                }
                break;
            }
            () = &mut persistence_run => break,
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
        }
    }
    drop(persistence_run);
    drop(node_run);
    background_tasks.shutdown().await;
    observability.shutdown().await;
    if let Some(managed) = managed {
        managed.hold_runtime_lock_until_process_exit();
    }
    if interface_failure.is_some() || node_failure {
        process::exit(1);
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
    let mut active_globals = active_plan.clone();
    active_globals.interfaces.clear();
    let mut replacement_globals = replacement.clone();
    replacement_globals.interfaces.clear();
    if active_globals != replacement_globals {
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
