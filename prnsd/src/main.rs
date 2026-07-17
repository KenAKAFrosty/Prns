//! The Personal Reticulum daemon: a configurable shared-instance node on the high-level [`Prns`]
//! runtime.
//!
//! It reads a stock RNS config the way a stock RNS user expects (`<dir>/config`, discovered along
//! RNS's own search order) and projects it onto a [`DaemonPlan`]. Then it elects its role on the
//! host's shared instance: with none running it becomes the instance — standing up the plan's
//! interfaces and serving the bus and control RPC for local apps (Sideband, NomadNet, MeshChat),
//! keyed on the node's own persistent identity; with one already running it defers, joining as a
//! client over that instance's bus and standing up none of its own, the honorable parity behavior a
//! stock RNS app follows. It forwards others' traffic when the config enables the transport role.

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The daemon is async
// glue around the engine; syscalls go through tokio/std, so no `unsafe`.
#![forbid(unsafe_code)]

mod cli;
mod construct;
mod identity;
mod interface_discovery;
#[cfg(feature = "otlp")]
mod metrics;
mod observability;
mod persist;
mod splash;
mod startup_progress;

use std::fmt;
use std::process::{self, ExitCode};

use personal_rns::config::{discover, plan, SharedInstance};
use personal_rns::identity::vault::FileVault;
use personal_rns::persistence::FileStore;
use personal_rns::routes;
use personal_rns::runtime::{
    boot_timeline_origin, Diagnostic, Manual, Prns, PrnsEvent, PrnsRecipe,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, RnsLocalBlackholeFile, Role,
    SharedInstanceCredentials, SharedInstanceEndpoint, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap;
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
    if let cli::Command::Run(args) = command {
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
    match run_command(command) {
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

fn run_command(command: cli::Command) -> Result<(), CommandError> {
    let paths = ServicePaths::discover()?;
    match command {
        cli::Command::Start(args) => start_or_attach(&paths, args),
        cli::Command::Restart(args) => {
            if prnsd_control::stop(&paths)? {
                eprintln!("Stopped prnsd");
            }
            start_new(&paths, args)
        }
        cli::Command::Stop => match prnsd_control::running(&paths)? {
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
        cli::Command::Status => match prnsd_control::running(&paths)? {
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
        cli::Command::Logs => match prnsd_control::running(&paths)? {
            Some(record) => attach(&paths, &record),
            None => Err(CommandError::NotRunning),
        },
        cli::Command::Run(_) => Ok(()),
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
    #[cfg(feature = "otlp")]
    let started = std::time::Instant::now();
    let observability = match observability::init(cli.log_format) {
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

    let discovered_config = match discover(cli.config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(event = "config_discovery_failed", error = %error);
            observability.shutdown().await;
            process::exit(1);
        }
    };
    let (config_text, config_source) = match &discovered_config.config {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => {
                tracing::info!(event = "config_loaded", path = %path.display());
                (text, path.display().to_string())
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
            (DEFAULT_CONFIG.to_string(), "<built-in config>".to_string())
        }
    };

    let reference = match personal_rns::config::reference::parse_named(&config_source, &config_text)
    {
        Ok(report) => {
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
            report.value
        }
        Err(errors) => {
            for diagnostic in errors.diagnostics() {
                match diagnostic.severity() {
                    personal_rns::config::ConfigSeverity::Warning => tracing::warn!(
                        event = "config_warning",
                        code = diagnostic.code().as_str(),
                        source = diagnostic.source(),
                        line = diagnostic.line(),
                        path = diagnostic.path(),
                        diagnostic = %diagnostic,
                    ),
                    personal_rns::config::ConfigSeverity::Error => tracing::error!(
                        event = "config_invalid",
                        code = diagnostic.code().as_str(),
                        source = diagnostic.source(),
                        line = diagnostic.line(),
                        path = diagnostic.path(),
                        diagnostic = %diagnostic,
                    ),
                }
            }
            observability.shutdown().await;
            process::exit(1);
        }
    };
    let plan = plan(&reference);
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
    let secret = identity::load_or_seed_transport_identity(&storage_dir);
    let shared_instance_credentials = SharedInstanceCredentials::from_identity_secret(&secret);
    let blackhole_file = RnsLocalBlackholeFile::new(storage_dir.join("blackhole"));
    let transport_secret = plan.transport.then(|| secret.clone());
    let shared_instance_secret = (!plan.transport
        && matches!(plan.shared_instance, SharedInstance::Enabled { .. }))
    .then(|| secret.clone());

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
        interface_discovery::publication::prepare(&plan, &secret, network_identity.as_ref())
            .unzip();
    let mut prns = Prns::new(PrnsRecipe {
        transport_identity: transport_secret,
        pre_configured_destinations: discovery_destination,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: move |event, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) = event {
                let _ = rotated_tx.send(destination);
            }
        },
    })
    .with_timeline_origin(timeline_origin);
    if let Some(secret) = shared_instance_secret {
        prns = match prns.with_shared_instance_identity(secret) {
            Ok(prns) => prns,
            Err(error) => {
                tracing::error!(event = "shared_instance_identity_failed", error = ?error);
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
                    constructed_interfaces =
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
            constructed_interfaces = construct::construct_interfaces(&prns_handle, &plan).await;
            owns_tables = true;
        }
    }

    let mut persistence = None;
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

    let discovery_task = if owns_tables {
        match prepared_discovery.take() {
            Some(discovery) => {
                let observer = discovery.observer();
                prns = prns.with_accepted_announce_observer(move |observation| {
                    observer.observe(observation);
                });
                let clock = prns.clock();
                Some(discovery.spawn(prns_handle.clone(), clock))
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
    if let Some(managed) = managed.as_ref() {
        if let Err(error) = managed.mark_ready() {
            tracing::error!(event = "managed_ready_failed", error = %error);
            observability.shutdown().await;
            process::exit(1);
        }
    }
    tokio::select! {
        () = prns.run() => {}
        () = persist::run_until_shutdown(persistence, managed.as_ref()) => {}
    }
    if let Some(discovery) = discovery_task {
        discovery.shutdown().await;
    }
    if let Some(publisher) = discovery_publication_task {
        if let Err(error) = publisher.shutdown().await {
            tracing::warn!(event = "interface_discovery_publisher_task_failed", error = %error);
        }
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
}
