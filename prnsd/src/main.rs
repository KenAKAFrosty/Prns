//! The Personal Reticulum daemon: a configurable shared-instance node on the high-level [`Prns`]
//! runtime.
//!
//! It reads a stock RNS config the way a stock RNS user expects (`<dir>/config`, discovered along
//! RNS's own search order) and projects it onto a [`DaemonPlan`]. Then it elects its role on the
//! host's shared instance: with none running it becomes the instance — standing up the plan's
//! interfaces and serving the bus and control RPC for local apps (Sideband, NomadNet, MeshChat),
//! keyed on the node's own persistent identity; with one already running it defers, joining as a
//! client over that instance's bus and standing up none of its own, the honorable parity behavior a
//! stock RNS app follows. It announces itself as `lxmf.delivery` so it surfaces as a messageable
//! peer, and forwards others' traffic when the config enables the transport role.

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The daemon is async
// glue around the engine; syscalls go through tokio/std, so no `unsafe`.
#![forbid(unsafe_code)]

mod cli;
mod construct;
mod identity;
#[cfg(feature = "otlp")]
mod metrics;
mod observability;
mod persist;
mod splash;
mod startup_progress;

use core::time::Duration;
use std::process;

use clap::Parser;

use personal_rns::config::{discover, plan, SharedInstance};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::vault::FileVault;
use personal_rns::persistence::FileStore;
use personal_rns::routes;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, Manual, PreConfiguredDestination, Prns, PrnsEvent,
    PrnsRecipe, TokioPrnsHandle,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, RnsLocalBlackholeFile, Role,
    SharedInstanceCredentials, SharedInstanceEndpoint, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;

/// The destination the daemon announces itself as: `lxmf.delivery`, the aspect LXMF apps
/// (Sideband/Columba) message — so the daemon surfaces as a real, messageable peer.
const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
/// The `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])` = `fixarray(2)` ‖
/// `bin8("Personal rnsd")` ‖ `nil` — the shape LXMF emits, so apps surface the display name (the
/// `\x0d` length byte = 13 = the name's length).
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x0dPersonal rnsd\xc0";

/// How often the daemon re-announces itself (RNS default 6h; the first fires immediately).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The default config when a host has none yet: a single LAN auto-interface and a shared instance,
/// the same starting point RNS writes on first run.
const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = No\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n";

async fn announce_loop(handle: TokioPrnsHandle, destination: DestinationHash) {
    let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        interval.tick().await;
        handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }));
    }
}

#[tokio::main]
async fn main() {
    #[cfg(feature = "otlp")]
    let started = std::time::Instant::now();
    let cli = cli::Cli::parse();
    let observability = match observability::init(cli.log_format) {
        Ok(observability) => observability,
        Err(error) => {
            eprintln!("prnsd observability initialization failed: {error}");
            process::exit(1);
        }
    };
    if cli.log_format == cli::LogFormat::Human {
        splash::print(concat!(
            "Personal Reticulum daemon · v",
            env!("CARGO_PKG_VERSION")
        ));
    }
    tracing::info!(
        event = "daemon_starting",
        version = env!("CARGO_PKG_VERSION"),
    );

    let discovered_config = discover(cli.config.as_deref());
    let config_text = match &discovered_config.reference {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => {
                tracing::info!(event = "config_loaded", path = %path.display());
                text
            }
            Err(error) => {
                tracing::error!(
                    event = "config_read_failed",
                    path = %path.display(),
                    error = %error,
                );
                observability.shutdown().await;
                process::exit(1);
            }
        },
        None => {
            tracing::info!(
                event = "config_defaulted",
                directory = %discovered_config.dir.display(),
            );
            DEFAULT_CONFIG.to_string()
        }
    };

    let reference = match personal_rns::config::reference::parse(&config_text) {
        Ok(reference) => reference,
        Err(error) => {
            tracing::error!(event = "config_parse_failed", error = %error);
            observability.shutdown().await;
            process::exit(1);
        }
    };
    let plan = plan(&reference);

    let storage_dir = discovered_config.dir.join("storage");
    let secret = identity::load_or_seed_transport_identity(&storage_dir);
    let shared_instance_credentials = SharedInstanceCredentials::from_identity_secret(&secret);
    let blackhole_file = RnsLocalBlackholeFile::new(storage_dir.join("blackhole"));
    let transport_secret = plan.transport.then(|| secret.clone());

    let announce_destination = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: ANNOUNCE_APP_NAME,
        aspects: ANNOUNCE_ASPECTS,
        identity: secret,
        announce_app_data: ANNOUNCE_APP_DATA,
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::Ratcheted,
    };
    let destination = announce_destination
        .destination_hash()
        .expect("the lxmf.delivery name is valid");

    let persist_dir = persist::store_dir(&storage_dir);
    let store = FileStore::new(&persist_dir);
    let timeline_origin = boot_timeline_origin(&store);
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut prns = Prns::new(PrnsRecipe {
        transport_identity: transport_secret,
        pre_configured_destinations: [announce_destination],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: move |event, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { .. }) = event {
                let _ = rotated_tx.send(());
            }
        },
    })
    .with_timeline_origin(timeline_origin);
    let prns_handle = prns.handle();

    // Elect this node's role on the host's shared instance before standing up any interfaces: a
    // client defers to the running instance and rides its bus, standing up none of its own.
    // Only a node that owns tables seeds and persists them (RNS gates persistence the same way
    // for shared-instance clients).
    let mut owns_tables = false;
    match plan.shared_instance {
        SharedInstance::Enabled {
            instance_port,
            control_port,
        } => {
            let mut ports = InstancePorts::default();
            if let Some(bus) = instance_port {
                ports.bus = bus;
            }
            if let Some(control) = control_port {
                ports.control = control;
            }
            match join_shared_instance(
                &prns_handle,
                SharedInstanceIntent {
                    credentials: shared_instance_credentials,
                    blackhole_file: blackhole_file.clone(),
                    ports,
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
                    );
                    construct::construct_interfaces(&prns_handle, &plan).await;
                    owns_tables = true;
                }
                Ok(Role::JoinedAsClient { of }) => {
                    tracing::info!(event = "shared_instance_joined");
                    tracing::debug!(event = "shared_instance_joined_detail", instance = %of);
                }
                Err(JoinError::InstanceAlreadyRunning { at }) => {
                    tracing::error!(event = "shared_instance_refused", endpoint = %at);
                    observability.shutdown().await;
                    process::exit(1);
                }
                Err(JoinError::InstanceBusUnavailable { endpoint, kind }) => {
                    let endpoint = match endpoint {
                        SharedInstanceEndpoint::TcpBus => "tcp_bus",
                        #[cfg(target_os = "linux")]
                        SharedInstanceEndpoint::AbstractUnixBus => "abstract_unix_bus",
                    };
                    tracing::error!(
                        event = "shared_instance_bus_unavailable",
                        endpoint,
                        error_kind = ?kind,
                    );
                    observability.shutdown().await;
                    process::exit(1);
                }
            }
        }
        SharedInstance::Disabled => {
            tracing::info!(event = "standalone_node_started");
            construct::construct_interfaces(&prns_handle, &plan).await;
            owns_tables = true;
        }
    }

    if owns_tables {
        let mut restore_progress = observability.state_restore_progress();
        let vault = FileVault::new(&persist_dir);
        let blackholes = match blackhole_file.load(
            shared_instance_credentials.transport_identity_hash,
            timeline_origin,
        ) {
            Ok(entries) => prns.seed_blackholed_identities(entries),
            Err(error) => {
                tracing::warn!(event = "blackhole_restore_failed", error = %error);
                Default::default()
            }
        };
        let routes = match restore_progress.as_mut() {
            Some(progress) => prns.seed_routes_from_store_reporting(&store, |route_progress| {
                progress.observe(route_progress);
            }),
            None => prns.seed_routes_from_store(&store),
        };
        let known_destinations = prns.seed_known_destinations_from_store(&store);
        let tunnels = prns.seed_tunnels_from_store(&store);
        let ratchets = prns.seed_self_ratchets_from_vault(&vault);
        if let Some(progress) = restore_progress {
            progress.finish();
        }
        tracing::info!(
            event = "state_restored",
            blackholes = blackholes.seeded_count,
            routes = routes.seeded_count,
            known_destinations = known_destinations.seeded_count,
            tunnels = tunnels.seeded_count,
            ratchets = ratchets.seeded_count,
            refused = blackholes.refused_count
                + routes.refused_count
                + known_destinations.refused_count
                + tunnels.refused_count
                + ratchets.refused_count,
            dropped = blackholes.dropped_count
                + routes.dropped_count
                + known_destinations.dropped_count
                + tunnels.dropped_count
                + ratchets.dropped_count,
        );
        tokio::spawn(persist::persist_loop(
            prns_handle.clone(),
            store,
            persist::PERSIST_INTERVAL,
        ));
        tokio::spawn(persist::ratchet_flush_loop(
            prns_handle.clone(),
            vault,
            rotated_rx,
        ));
    }

    tokio::spawn(announce_loop(prns_handle.clone(), destination));
    #[cfg(feature = "otlp")]
    let metrics_task = observability.metrics_reporter().map(|reporter| {
        let runtime_up = reporter.runtime_up_handle();
        (
            tokio::spawn(reporter.run(prns_handle.clone(), started)),
            runtime_up,
        )
    });

    tracing::info!(
        event = "daemon_ready",
        transport = plan.transport,
        deferred_interfaces = plan.deferred.len(),
    );
    tokio::select! {
        () = prns.run() => {}
        () = persist::flush_on_shutdown(
            prns_handle.clone(),
            owns_tables.then(|| persist_dir.clone()),
        ) => {}
    }
    #[cfg(feature = "otlp")]
    if let Some((task, runtime_up)) = metrics_task {
        task.abort();
        let _ = task.await;
        runtime_up.record(0, &[]);
    }
    observability.shutdown().await;
}
