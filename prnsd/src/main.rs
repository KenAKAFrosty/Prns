//! The Personal Reticulum daemon: a configurable shared-instance node built on [`PrnsNode`].
//!
//! It reads a stock RNS config the way a stock RNS user expects (`<dir>/config`, discovered along
//! RNS's own search order) and projects it onto a [`personal_rns::config::DaemonPlan`]. Then it elects its role on the
//! host's shared instance: with none running it becomes the instance — standing up the plan's
//! interfaces and serving the bus and control RPC for local apps (Sideband, NomadNet, MeshChat),
//! keyed on the node's own persistent identity; with one already running it defers, joining as a
//! client over that instance's bus and standing up none of its own, the honorable parity behavior a
//! stock RNS app follows. It forwards others' traffic when the config enables the transport role.

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The daemon is async
// glue around the engine; syscalls go through tokio/std, so no `unsafe`.
#![forbid(unsafe_code)]

mod blackhole_exchange;
mod cli;
mod construct;
mod identity;
mod i2p_doctor;
mod interface_discovery;
mod management_announces;
#[cfg(feature = "otlp")]
mod metrics;
mod observability;
mod persist;
mod probe_responder;
mod remote_management;
mod request_services;
mod splash;
mod startup_progress;

use std::fmt;
use std::process::{self, ExitCode};

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
use prnsd_control::{
    LaunchSpec, LogLane, ManagedProcess, ServiceError, ServicePaths, ServiceRecord, ServiceState,
    StartOutcome, StateDirectoryError,
};

const DAEMON_SUBTITLE: &str = concat!("Personal Reticulum daemon · v", env!("CARGO_PKG_VERSION"));

const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n";

#[derive(Debug)]
enum CommandError {
    StateDirectory(StateDirectoryError),
    CurrentExecutable(std::io::Error),
    CurrentDirectory(std::io::Error),
    Service(ServiceError),
    NotRunning,
}

enum ManagedCommand {
    Start(cli::LaunchArgs),
    Restart(cli::LaunchArgs),
    Stop,
    Status,
    Logs,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::CurrentExecutable(error) => {
                write!(formatter, "Could not locate the prnsd executable: {error}")
            }
            Self::CurrentDirectory(error) => {
                write!(
                    formatter,
                    "Could not determine the current directory: {error}"
                )
            }
            Self::Service(error) => error.fmt(formatter),
            Self::NotRunning => formatter.write_str("prnsd is not running"),
        }
    }
}

impl From<StateDirectoryError> for CommandError {
    fn from(error: StateDirectoryError) -> Self {
        Self::StateDirectory(error)
    }
}

impl From<ServiceError> for CommandError {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args.get(1).is_some_and(|arg| arg == "--print-banner") {
        splash::print(DAEMON_SUBTITLE);
        return ExitCode::SUCCESS;
    }
    let command = match cli::parse_from(args) {
        Ok(command) => command,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(exit_code.clamp(0, 255) as u8);
        }
    };
    let managed_command = match command {
        cli::Command::Run(args) => {
            let managed = match ManagedProcess::from_environment() {
                Ok(managed) => managed,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            run_daemon(args, managed).await;
            return ExitCode::SUCCESS;
        }
        cli::Command::I2p(args) => return run_i2p_command(args).await,
        cli::Command::Start(args) => ManagedCommand::Start(args),
        cli::Command::Restart(args) => ManagedCommand::Restart(args),
        cli::Command::Stop => ManagedCommand::Stop,
        cli::Command::Status => ManagedCommand::Status,
        cli::Command::Logs => ManagedCommand::Logs,
    };
    match run_command(managed_command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("prnsd: {error}");
            if matches!(error, CommandError::NotRunning) {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

async fn run_i2p_command(args: cli::I2pArgs) -> ExitCode {
    match args.command {
        cli::I2pCommand::Doctor(args) => {
            let remote_access = if args.allow_remote_sam {
                i2p_doctor::RemoteSamAccess::ExplicitlyAllowed
            } else {
                i2p_doctor::RemoteSamAccess::LoopbackOnly
            };
            let request = i2p_doctor::I2pDoctorRequest::new(args.sam_bridge, remote_access);
            match i2p_doctor::run(request).await {
                Ok(ready) => {
                    println!("{ready}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run_command(command: ManagedCommand) -> Result<(), CommandError> {
    let paths = ServicePaths::discover()?;
    match command {
        ManagedCommand::Start(args) => start_or_attach(&paths, args),
        ManagedCommand::Restart(args) => {
            if prnsd_control::stop(&paths)? {
                eprintln!("Stopped prnsd");
            }
            start_new(&paths, args)
        }
        ManagedCommand::Stop => match prnsd_control::running(&paths)? {
            Some(record) => {
                print_managed_banner(&record);
                eprintln!(
                    "Stopping prnsd (pid {}); showing recent and shutdown logs\n",
                    record.pid
                );
                prnsd_control::stop_and_follow(&paths, &record)?;
                eprintln!("\nStopped prnsd");
                Ok(())
            }
            None => {
                eprintln!("prnsd is already stopped");
                Ok(())
            }
        },
        ManagedCommand::Status => match prnsd_control::running(&paths)? {
            Some(record) => {
                let state = match record.state {
                    ServiceState::Starting => "starting",
                    ServiceState::Running => "running",
                };
                eprintln!(
                    "prnsd is {state} (pid {}, version {}, log {})",
                    record.pid,
                    record.version,
                    record.log(&paths).display()
                );
                Ok(())
            }
            None => Err(CommandError::NotRunning),
        },
        ManagedCommand::Logs => match prnsd_control::running(&paths)? {
            Some(record) => attach(&paths, &record),
            None => Err(CommandError::NotRunning),
        },
    }
}

fn start_or_attach(paths: &ServicePaths, args: cli::LaunchArgs) -> Result<(), CommandError> {
    if let Some(record) = prnsd_control::running(paths)? {
        eprintln!("prnsd is already running (pid {})", record.pid);
        let signature = daemon_signature(&args.daemon);
        if explicit_launch_configuration(&args.daemon) && record.signature != signature {
            eprintln!("Existing launch options were retained; use prnsd restart to replace them");
        }
        if args.detach {
            if record.state == ServiceState::Starting {
                prnsd_control::wait_until_ready(paths, record)?;
            }
            return Ok(());
        }
        return attach(paths, &record);
    }
    start_new(paths, args)
}

fn start_new(paths: &ServicePaths, args: cli::LaunchArgs) -> Result<(), CommandError> {
    let binary = std::env::current_exe().map_err(CommandError::CurrentExecutable)?;
    let working_dir = std::env::current_dir().map_err(CommandError::CurrentDirectory)?;
    let daemon_args = args.daemon.command_line();
    #[cfg(windows)]
    let managed_binary = paths.state_dir.join("prnsd-managed.exe");
    let log_lane = match args.daemon.log_format {
        cli::LogFormat::Human => LogLane::Human,
        cli::LogFormat::Json => LogLane::Json,
    };
    eprintln!("Starting prnsd...");
    let outcome = prnsd_control::start(
        paths,
        LaunchSpec {
            binary: &binary,
            #[cfg(windows)]
            managed_binary: Some(&managed_binary),
            #[cfg(not(windows))]
            managed_binary: None,
            args: &daemon_args,
            working_dir: &working_dir,
            log_lane,
            signature: daemon_signature(&args.daemon),
            version: env!("CARGO_PKG_VERSION"),
        },
    );
    let record = match outcome {
        Ok(StartOutcome::Started(record)) => {
            eprintln!(
                "Started prnsd (pid {}, log {})",
                record.pid,
                record.log(paths).display()
            );
            record
        }
        Ok(StartOutcome::AlreadyRunning(record)) => {
            eprintln!("prnsd is already running (pid {})", record.pid);
            record
        }
        Err(ServiceError::ProcessExited { log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(ServiceError::ProcessExited { log }.into());
        }
        Err(ServiceError::StartupTimedOut { pid, log }) => {
            let _ = prnsd_control::print_recent_log(&log);
            return Err(ServiceError::StartupTimedOut { pid, log }.into());
        }
        Err(error) => return Err(error.into()),
    };
    if args.detach {
        return Ok(());
    }
    attach(paths, &record)
}

fn attach(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), CommandError> {
    print_managed_banner(record);
    eprintln!("Attached to prnsd; Ctrl-C detaches without stopping the daemon\n");
    prnsd_control::follow(paths, record).map_err(CommandError::from)
}

fn print_managed_banner(record: &ServiceRecord) {
    splash::print(&format!("Personal Reticulum daemon · v{}", record.version));
}

fn daemon_signature(args: &cli::DaemonArgs) -> u64 {
    prnsd_control::launch_signature(args.command_line(), std::env::vars_os())
}

fn explicit_launch_configuration(args: &cli::DaemonArgs) -> bool {
    args.has_explicit_options()
        || std::env::vars_os().any(|(name, _)| {
            name == "RUST_LOG" || name.to_str().is_some_and(|name| name.starts_with("OTEL_"))
        })
}

async fn run_daemon(cli: cli::DaemonArgs, managed: Option<ManagedProcess>) {
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
        splash::print(DAEMON_SUBTITLE);
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

    // Elect this node's role on the host's shared instance before standing up any interfaces: a
    // client defers to the running instance and rides its bus, standing up none of its own.
    // Only a node that owns tables seeds and persists them (RNS gates persistence the same way
    // for shared-instance clients).
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
        failed = wait_for_interface_failure(
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

async fn wait_for_interface_failure(
    handle: &personal_rns::runtime::PrnsNodeHandle,
    expected: tokio::sync::watch::Receiver<
        std::collections::BTreeSet<personal_rns::interfaces::InterfaceId>,
    >,
    enabled: bool,
) -> personal_rns::interfaces::InterfaceId {
    if !enabled {
        return std::future::pending().await;
    }
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
    loop {
        interval.tick().await;
        let current = handle
            .interfaces()
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot.connection))
            .collect::<Vec<_>>();
        let expected = expected.borrow().iter().copied().collect::<Vec<_>>();
        if let Some(failed) = first_interface_failure(&expected, &current) {
            return failed;
        }
    }
}

fn first_interface_failure(
    expected: &[personal_rns::interfaces::InterfaceId],
    current: &[(
        personal_rns::interfaces::InterfaceId,
        personal_rns::interfaces::ConnectionState,
    )],
) -> Option<personal_rns::interfaces::InterfaceId> {
    current
        .iter()
        .find_map(|(id, connection)| {
            (*connection == personal_rns::interfaces::ConnectionState::Failed).then_some(*id)
        })
        .or_else(|| {
            expected
                .iter()
                .copied()
                .find(|expected| !current.iter().any(|(current, _)| current == expected))
        })
}

#[cfg(test)]
mod interface_failure_tests {
    use super::first_interface_failure;
    use personal_rns::interfaces::{ConnectionState, InterfaceId};

    #[test]
    fn failure_detection_covers_failed_and_departed_initial_interfaces() {
        let first = InterfaceId::new([1; 8]);
        let second = InterfaceId::new([2; 8]);
        let expected = [first, second];

        assert_eq!(
            first_interface_failure(
                &expected,
                &[
                    (first, ConnectionState::Connected),
                    (second, ConnectionState::Reconnecting),
                ],
            ),
            None
        );
        assert_eq!(
            first_interface_failure(
                &expected,
                &[
                    (first, ConnectionState::Connected),
                    (second, ConnectionState::Failed),
                ],
            ),
            Some(second)
        );
        assert_eq!(
            first_interface_failure(&expected, &[(first, ConnectionState::Connected)]),
            Some(second)
        );
    }
}
