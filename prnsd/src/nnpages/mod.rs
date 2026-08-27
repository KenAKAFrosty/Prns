use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use personal_rns::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::runtime::request_endpoints::{Decline, RequestContext};
use personal_rns::runtime::{PrnsNodeHandle, RuntimeRequestHandlerError};
use personal_rns::wire::DestinationHash;

use crate::services::DaemonRequestState;

mod control;
mod settings;

use control::{atomic_control_write, request_announce, request_refresh};

pub(crate) use control::{
    next_control_request, recover_control_state, NnPagesControlFailure, NnPagesControlKind,
};

pub(crate) use settings::{
    NnPagesSettings, NnPagesSettingsSnapshot, NnPagesSettingsStatus,
    DEFAULT_ANNOUNCE_INTERVAL_MINUTES, DEFAULT_SETTINGS_DOCUMENT, SETTINGS_FILE_NAME,
};

pub(crate) const DIRECTORY_NAME: &str = "nnpages";
pub(crate) const PAGES_DIRECTORY_NAME: &str = "pages";
pub(crate) const FILES_DIRECTORY_NAME: &str = "files";
pub(crate) const INDEX_FILE_NAME: &str = "index.mu";
pub(crate) const NODE_NAME_FILE_NAME: &str = "name";
pub(crate) const SOURCE_PAGE_FILE_NAME: &str = "source.mu";
pub(crate) const COMING_FROM_RNS_PAGE_FILE_NAME: &str = "coming-from-rns.mu";
pub(crate) const SOURCE_ARCHIVE_FILE_NAME: &str = "source.zip";
pub(crate) const SOURCE_CHECKSUM_FILE_NAME: &str = "source.zip.sha256";
pub(crate) const DEFAULT_INDEX_PAGE: &[u8] = concat!(
    include_str!("../../../assets/nnpages/masthead.mu"),
    include_str!("../../assets/nnpages/index_welcome.mu"),
    include_str!("../../../assets/nnpages/nav.mu"),
    include_str!("../../../assets/nnpages/why_prns.mu"),
    include_str!("../../../assets/nnpages/license.mu"),
    include_str!("../../../assets/nnpages/quote.mu"),
    include_str!("../../assets/nnpages/index_outro.mu"),
    include_str!("../../../assets/nnpages/credits.mu"),
)
.as_bytes();

const REQUEST_PREFIX: &str = "/page/";
const FILE_REQUEST_PREFIX: &str = "/file/";
pub(crate) const MAX_PAGE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SERVED_NAME_BYTES: usize = u8::MAX as usize;
const MAX_HOSTED_DEPTH: usize = 32;
const MAX_HOSTED_ROUTES: usize = 4096;
const MAX_SCAN_ENTRIES: usize = 65_536;

#[derive(Clone)]
pub(crate) struct NnPagesCatalog {
    config_dir: Arc<PathBuf>,
    root: Arc<PathBuf>,
    pages_root: Arc<PathBuf>,
    files_root: Arc<PathBuf>,
    routes: Arc<RwLock<Arc<Vec<HostedRoute>>>>,
    settings: Arc<RwLock<NnPagesSettingsSnapshot>>,
    settings_sender: Arc<tokio::sync::watch::Sender<NnPagesSettings>>,
    reconciliation: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostedKind {
    Page,
    File,
}

impl HostedKind {
    const fn request_prefix(self) -> &'static str {
        match self {
            Self::Page => REQUEST_PREFIX,
            Self::File => FILE_REQUEST_PREFIX,
        }
    }

    const fn max_bytes(self) -> u64 {
        match self {
            Self::Page => MAX_PAGE_BYTES,
            Self::File => MAX_FILE_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostedRoute {
    request_path: String,
    path_hash: RequestPathHash,
    relative_path: PathBuf,
    kind: HostedKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NnPagesRefreshReport {
    pub(crate) discovered: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) unchanged: usize,
    pub(crate) settings_status: NnPagesSettingsStatus,
    pub(crate) settings_changed: bool,
}

#[derive(Debug)]
pub(crate) enum NnPagesRefreshError {
    Scan(io::Error),
    SourcePage(Box<crate::daemon::configuration::ServerBootstrapError>),
    Runtime {
        operation: &'static str,
        path: String,
        source: RuntimeRequestHandlerError,
    },
    CatalogPoisoned,
    DestinationUnavailable,
}

#[derive(Debug)]
pub(crate) enum NnPagesCliError {
    CommandContext(crate::command_context::CommandContextError),
    Control(io::Error),
    TimedOut,
    OperationFailed {
        kind: NnPagesControlKind,
        failure: NnPagesControlFailure,
    },
    OperationAborted(NnPagesControlKind),
    OperationIndeterminate(NnPagesControlKind),
    Seed(crate::daemon::configuration::ServerBootstrapError),
    InvalidName,
    NameTooLong,
    InvalidResult,
}

impl core::fmt::Display for NnPagesCliError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CommandContext(error) => error.fmt(formatter),
            Self::Control(error) => write!(formatter, "NNPages control failed: {error}"),
            Self::TimedOut => write!(
                formatter,
                "the daemon did not acknowledge the request within {} seconds",
                control::CONTROL_TIMEOUT.as_secs(),
            ),
            Self::OperationFailed { kind, failure } => {
                write!(formatter, "the daemon could not {}: {}", kind.action(), failure.description())
            }
            Self::OperationAborted(kind) => {
                write!(formatter, "the daemon aborted the NNPages {} before it could acknowledge completion; retry the request", kind.noun())
            }
            Self::OperationIndeterminate(NnPagesControlKind::Refresh) => {
                formatter.write_str("the daemon restarted while the NNPages refresh was in flight; its outcome is unknown, and it is safe to retry")
            }
            Self::OperationIndeterminate(NnPagesControlKind::Announce) => {
                formatter.write_str("the daemon restarted while the NNPages announcement was in flight; it may already have aired, so retrying may send a duplicate")
            }
            Self::Seed(error) => write!(formatter, "could not seed the starter page: {error}"),
            Self::InvalidName => {
                formatter.write_str("the node name must be one non-empty line of text")
            }
            Self::NameTooLong => write!(
                formatter,
                "the node name must be at most {MAX_ANNOUNCE_APP_DATA_LEN} bytes"
            ),
            Self::InvalidResult => formatter.write_str("the daemon returned an invalid result"),
        }
    }
}

impl std::error::Error for NnPagesCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CommandContext(error) => Some(error),
            Self::Control(error) => Some(error),
            Self::Seed(error) => Some(error),
            Self::TimedOut
            | Self::OperationFailed { .. }
            | Self::OperationAborted(_)
            | Self::OperationIndeterminate(_)
            | Self::InvalidName
            | Self::NameTooLong
            | Self::InvalidResult => None,
        }
    }
}

impl core::fmt::Display for NnPagesRefreshError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Scan(error) => {
                write!(formatter, "could not scan the hosted directories: {error}")
            }
            Self::SourcePage(source) => {
                write!(formatter, "could not re-render the source page: {source}")
            }
            Self::Runtime {
                operation,
                path,
                source,
            } => {
                write!(
                    formatter,
                    "could not {operation} node request route {path}: {source}"
                )
            }
            Self::CatalogPoisoned => formatter.write_str("the page catalog lock was poisoned"),
            Self::DestinationUnavailable => {
                formatter.write_str("this daemon does not own the hosted page destination")
            }
        }
    }
}

impl std::error::Error for NnPagesRefreshError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scan(error) => Some(error),
            Self::SourcePage(source) => Some(source),
            Self::Runtime { source, .. } => Some(source),
            Self::CatalogPoisoned | Self::DestinationUnavailable => None,
        }
    }
}

impl NnPagesCatalog {
    pub(crate) fn discover(config_dir: &Path) -> io::Result<Self> {
        let root = root(config_dir);
        let pages_root = page_root(config_dir);
        let files_root = file_root(config_dir);
        let routes = scan_routes(&pages_root, &files_root)?;
        Ok(Self::new(
            config_dir.to_path_buf(),
            root,
            pages_root,
            files_root,
            routes,
        ))
    }

    pub(crate) fn empty(config_dir: &Path) -> Self {
        Self::new(
            config_dir.to_path_buf(),
            root(config_dir),
            page_root(config_dir),
            file_root(config_dir),
            Vec::new(),
        )
    }

    fn new(
        config_dir: PathBuf,
        root: PathBuf,
        pages_root: PathBuf,
        files_root: PathBuf,
        routes: Vec<HostedRoute>,
    ) -> Self {
        let settings = settings::load(&root);
        log_settings_snapshot(&settings, "startup");
        let (settings_sender, _) = tokio::sync::watch::channel(settings.effective());
        Self {
            config_dir: Arc::new(config_dir),
            root: Arc::new(root),
            pages_root: Arc::new(pages_root),
            files_root: Arc::new(files_root),
            routes: Arc::new(RwLock::new(Arc::new(routes))),
            settings: Arc::new(RwLock::new(settings)),
            settings_sender: Arc::new(settings_sender),
            reconciliation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn request_paths(&self) -> Vec<String> {
        self.snapshot()
            .map(|routes| {
                routes
                    .iter()
                    .map(|route| route.request_path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn index_path(&self) -> PathBuf {
        self.pages_root.join(INDEX_FILE_NAME)
    }

    pub(crate) fn node_name_path(&self) -> PathBuf {
        self.root.join(NODE_NAME_FILE_NAME)
    }

    pub(crate) fn announcement_settings(&self) -> tokio::sync::watch::Receiver<NnPagesSettings> {
        self.settings_sender.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn settings_snapshot(&self) -> Option<NnPagesSettingsSnapshot> {
        self.settings.read().ok().map(|settings| settings.clone())
    }

    pub(crate) async fn refresh(
        &self,
        handle: &PrnsNodeHandle,
        destination: DestinationHash,
    ) -> Result<NnPagesRefreshReport, NnPagesRefreshError> {
        use crate::daemon::configuration::{
            refresh_source_page, SourcePageRefresh, SourcePageState,
        };

        let _guard = self.reconciliation.lock().await;
        let config_dir = Arc::clone(&self.config_dir);
        let root = Arc::clone(&self.root);
        let pages_root = Arc::clone(&self.pages_root);
        let files_root = Arc::clone(&self.files_root);
        let (source_page, discovered, settings) = tokio::task::spawn_blocking(move || {
            (
                refresh_source_page(&config_dir),
                scan_routes(&pages_root, &files_root),
                settings::load(&root),
            )
        })
        .await
        .map_err(|error| {
            NnPagesRefreshError::Scan(io::Error::other(format!(
                "route scanner task failed: {error}"
            )))
        })?;
        match source_page.map_err(|source| NnPagesRefreshError::SourcePage(Box::new(source)))? {
            SourcePageRefresh::Rewritten(SourcePageState::ArchiveMissing) => {
                tracing::info!(
                    event = "nnpages_source_page_rerendered",
                    archive = "missing"
                );
            }
            SourcePageRefresh::Rewritten(SourcePageState::ArchiveStaged {
                archive_bytes, ..
            }) => {
                tracing::info!(
                    event = "nnpages_source_page_rerendered",
                    archive = "staged",
                    archive_bytes,
                );
            }
            SourcePageRefresh::Unchanged
            | SourcePageRefresh::OperatorOwned
            | SourcePageRefresh::Absent => {}
        }
        let discovered = discovered.map_err(NnPagesRefreshError::Scan)?;
        let current = self
            .snapshot()
            .ok_or(NnPagesRefreshError::CatalogPoisoned)?;
        let added = discovered
            .iter()
            .filter(|candidate| {
                !current
                    .iter()
                    .any(|route| route.request_path == candidate.request_path)
            })
            .cloned()
            .collect::<Vec<_>>();
        let removed = current
            .iter()
            .filter(|candidate| {
                !discovered
                    .iter()
                    .any(|route| route.request_path == candidate.request_path)
            })
            .cloned()
            .collect::<Vec<_>>();

        let unchanged = discovered
            .iter()
            .filter(|candidate| {
                current
                    .iter()
                    .any(|route| route.request_path == candidate.request_path)
            })
            .count();
        let mut published = current.as_ref().clone();
        for route in &added {
            handle
                .register_request_path(destination, &route.request_path, RequestPolicy::AllowAll)
                .await
                .map_err(|source| NnPagesRefreshError::Runtime {
                    operation: "register",
                    path: route.request_path.clone(),
                    source,
                })?;
            published.push(route.clone());
            published.sort_by(|left, right| left.request_path.cmp(&right.request_path));
            self.publish_routes(published.clone())?;
        }

        for route in &removed {
            handle
                .unregister_request_path(destination, &route.request_path)
                .await
                .map_err(|source| NnPagesRefreshError::Runtime {
                    operation: "unregister",
                    path: route.request_path.clone(),
                    source,
                })?;
            published.retain(|candidate| candidate.request_path != route.request_path);
            self.publish_routes(published.clone())?;
        }
        self.publish_routes(discovered.clone())?;
        let (settings_status, settings_changed) = self.publish_settings(settings)?;
        Ok(NnPagesRefreshReport {
            discovered: discovered.len(),
            added: added.len(),
            removed: removed.len(),
            unchanged,
            settings_status,
            settings_changed,
        })
    }

    pub(crate) async fn respond(
        &self,
        mut context: RequestContext<'_, DaemonRequestState>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        let routes = self.snapshot().ok_or(Decline::Ignore)?;
        let Some(route) = routes.iter().find(|route| route.path_hash == path_hash) else {
            return Err(Decline::Ignore);
        };
        let kind = route.kind;
        let root = match kind {
            HostedKind::Page => Arc::clone(&self.pages_root),
            HostedKind::File => Arc::clone(&self.files_root),
        };
        let relative_path = route.relative_path.clone();
        let request_path = route.request_path.clone();
        match tokio::task::spawn_blocking(move || {
            open_hosted(&root, &relative_path, kind.max_bytes())
        })
        .await
        {
            Ok(Ok(opened)) => match kind {
                HostedKind::Page => context.respond_open_bytes(opened.file, opened.byte_len),
                HostedKind::File => {
                    let Some(name) = route.request_path.strip_prefix(FILE_REQUEST_PREFIX) else {
                        return Err(Decline::Ignore);
                    };
                    context.respond_open_file(name, opened.file, opened.byte_len)
                }
            },
            Ok(Err(HostedReadError::Unavailable)) => Err(Decline::Ignore),
            Ok(Err(HostedReadError::TooLarge)) => {
                tracing::warn!(
                    event = "hosted_route_too_large",
                    path = request_path,
                    maximum_bytes = kind.max_bytes(),
                );
                Err(Decline::Ignore)
            }
            Ok(Err(HostedReadError::Read(error))) => {
                tracing::warn!(
                    event = "hosted_route_read_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
            Err(error) => {
                tracing::warn!(
                    event = "hosted_route_reader_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
        }
    }

    fn snapshot(&self) -> Option<Arc<Vec<HostedRoute>>> {
        self.routes.read().ok().map(|routes| Arc::clone(&routes))
    }

    fn publish_routes(&self, routes: Vec<HostedRoute>) -> Result<(), NnPagesRefreshError> {
        let mut published = self
            .routes
            .write()
            .map_err(|_| NnPagesRefreshError::CatalogPoisoned)?;
        *published = Arc::new(routes);
        Ok(())
    }

    fn publish_settings(
        &self,
        replacement: NnPagesSettingsSnapshot,
    ) -> Result<(NnPagesSettingsStatus, bool), NnPagesRefreshError> {
        let mut current = self
            .settings
            .write()
            .map_err(|_| NnPagesRefreshError::CatalogPoisoned)?;
        let source_changed = *current != replacement;
        let effective_changed = current.effective() != replacement.effective();
        if source_changed {
            log_settings_snapshot(&replacement, "refresh");
        }
        let status = replacement.status();
        let effective = replacement.effective();
        *current = replacement;
        drop(current);
        if effective_changed {
            self.settings_sender.send_replace(effective);
        }
        Ok((status, effective_changed))
    }
}

fn log_settings_snapshot(settings: &NnPagesSettingsSnapshot, cause: &'static str) {
    if let Some(error) = settings.diagnostic() {
        tracing::warn!(
            event = "nnpages_settings_defaulted",
            cause,
            error = %error,
            "NNPages settings are invalid or unreadable; using defaults"
        );
        return;
    }
    let effective = settings.effective();
    tracing::info!(
        event = "nnpages_settings_loaded",
        cause,
        source = settings.status().as_control_value(),
        announce = effective.announce(),
        announce_interval_minutes = effective.announce_interval_minutes(),
    );
}

pub(crate) async fn run_cli(args: crate::cli::NnPagesArgs) -> Result<(), NnPagesCliError> {
    match args.command {
        crate::cli::NnPagesCommand::Refresh(args) => {
            let discovered = discover_cli_config(args.config.as_deref())?;
            let report = request_refresh(&discovered.dir).await?;
            print_refresh_report(&report);
            Ok(())
        }
        crate::cli::NnPagesCommand::Seed(args) => {
            use crate::daemon::configuration::{
                format_archive_size, materialize_nnpages_settings, prepare_nnpages_layout,
                seed_coming_from_rns_page, seed_default_page, seed_source_page,
                stage_bundled_source, stage_source_archive, ManagedPageSeed, SourcePageSeed,
                SourcePageState,
            };

            let discovered = discover_cli_config(args.config.as_deref())?;
            prepare_nnpages_layout(&discovered.dir).map_err(NnPagesCliError::Seed)?;
            let seeded_settings =
                materialize_nnpages_settings(&discovered.dir, DEFAULT_SETTINGS_DOCUMENT)
                    .map_err(NnPagesCliError::Seed)?;
            match &seeded_settings {
                Some(path) => println!("Seeded {}.", path.display()),
                None => println!("settings.toml already exists; left untouched."),
            }
            if args.source {
                let staged = match args.source_archive.as_deref() {
                    Some(source) => stage_source_archive(&discovered.dir, source),
                    None => stage_bundled_source(&discovered.dir, true).and_then(|seed| {
                        seed.ok_or_else(|| {
                            crate::daemon::configuration::ServerBootstrapError::SourceArchiveUnavailable {
                                searched: Vec::new(),
                            }
                        })
                    }),
                }
                .map_err(NnPagesCliError::Seed)?;
                let action = if !staged.replaced.is_empty() {
                    "Updated"
                } else if staged.created.is_empty() {
                    "Verified"
                } else {
                    "Staged"
                };
                println!(
                    "{action} {} ({}); checksum available at files/{SOURCE_CHECKSUM_FILE_NAME}.",
                    staged.archive_path.display(),
                    format_archive_size(staged.archive_bytes),
                );
            } else {
                println!(
                    "Source archive not requested; run `prnsd nnpages seed --source` to stage it."
                );
            }
            let seeded_index = seed_default_page(&discovered.dir).map_err(NnPagesCliError::Seed)?;
            match &seeded_index {
                Some(path) => println!("Seeded {}.", path.display()),
                None => println!("index.mu already exists; left untouched."),
            }
            let source_page = seed_source_page(&discovered.dir).map_err(NnPagesCliError::Seed)?;
            match &source_page {
                SourcePageSeed::Written {
                    path,
                    state: SourcePageState::ArchiveMissing,
                } => println!(
                    "Seeded {}; it notes no source archive is staged yet.",
                    path.display()
                ),
                SourcePageSeed::Written {
                    path,
                    state: SourcePageState::ArchiveStaged { archive_bytes, .. },
                } => println!(
                    "Seeded {}; it serves files/{SOURCE_ARCHIVE_FILE_NAME} ({}).",
                    path.display(),
                    format_archive_size(*archive_bytes)
                ),
                SourcePageSeed::Unchanged(_) => {
                    println!("{SOURCE_PAGE_FILE_NAME} already current; left untouched.");
                }
                SourcePageSeed::OperatorOwned => {
                    println!("{SOURCE_PAGE_FILE_NAME} is operator-edited; left untouched.");
                }
            }
            let coming_from_rns =
                seed_coming_from_rns_page(&discovered.dir).map_err(NnPagesCliError::Seed)?;
            match &coming_from_rns {
                ManagedPageSeed::Written(path) => println!("Seeded {}.", path.display()),
                ManagedPageSeed::Unchanged => {
                    println!("{COMING_FROM_RNS_PAGE_FILE_NAME} already current; left untouched.");
                }
                ManagedPageSeed::OperatorOwned => {
                    println!(
                        "{COMING_FROM_RNS_PAGE_FILE_NAME} is operator-edited; left untouched."
                    );
                }
            }
            if seed_requires_refresh(
                seeded_settings.is_some(),
                seeded_index.is_some(),
                matches!(source_page, SourcePageSeed::Written { .. }),
                matches!(coming_from_rns, ManagedPageSeed::Written(_)),
            ) {
                match request_refresh(&discovered.dir).await {
                    Ok(report) => print_refresh_report(&report),
                    Err(NnPagesCliError::TimedOut) => println!(
                        "No running daemon acknowledged; the pages are on disk and register at the next reconciliation or start."
                    ),
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }
        crate::cli::NnPagesCommand::Announce(args) => {
            let discovered = discover_cli_config(args.config.as_deref())?;
            request_announce(&discovered.dir).await?;
            println!("Announced the hosted page destination on all interfaces.");
            Ok(())
        }
        crate::cli::NnPagesCommand::Rename(args) => {
            let name = validate_node_name(&args.name).map_err(|error| match error {
                NodeNameValidationError::Invalid => NnPagesCliError::InvalidName,
                NodeNameValidationError::TooLong => NnPagesCliError::NameTooLong,
            })?;
            let discovered = discover_cli_config(args.config.as_deref())?;
            let root = root(&discovered.dir);
            prepare_operator_root(&root).map_err(NnPagesCliError::Control)?;
            atomic_control_write(&root.join(NODE_NAME_FILE_NAME), name.as_bytes())
                .map_err(NnPagesCliError::Control)?;
            println!("Renamed the announced node to \"{name}\".");
            if !is_page_available(&page_root(&discovered.dir).join(INDEX_FILE_NAME)) {
                println!(
                    "Immediate announcement unavailable: nnpages/pages/index.mu is not serveable; the name is saved."
                );
                return Ok(());
            }
            match request_announce(&discovered.dir).await {
                Ok(()) => println!("Announced the new name on all interfaces."),
                Err(NnPagesCliError::TimedOut) => println!(
                    "Immediate announcement deferred: no running daemon acknowledged; the name is saved for the next announce."
                ),
                Err(error) => println!(
                    "Immediate announcement deferred: {error}; the name remains saved."
                ),
            }
            Ok(())
        }
    }
}

const fn seed_requires_refresh(
    settings_created: bool,
    index_created: bool,
    source_page_changed: bool,
    coming_from_rns_changed: bool,
) -> bool {
    settings_created || index_created || source_page_changed || coming_from_rns_changed
}

fn discover_cli_config(
    explicit: Option<&Path>,
) -> Result<prns_config::DiscoveredConfig, NnPagesCliError> {
    crate::command_context::discover(explicit).map_err(NnPagesCliError::CommandContext)
}

fn print_refresh_report(report: &NnPagesRefreshReport) {
    println!(
        "Refreshed {} hosted route(s): {} added, {} removed, {} unchanged.",
        report.discovered, report.added, report.removed, report.unchanged,
    );
    match report.settings_status {
        NnPagesSettingsStatus::Loaded if report.settings_changed => {
            println!("Applied changed NNPages settings.");
        }
        NnPagesSettingsStatus::Loaded => println!("NNPages settings are current."),
        NnPagesSettingsStatus::MissingDefaults => {
            println!("settings.toml is absent; using NNPages defaults.");
        }
        NnPagesSettingsStatus::InvalidDefaults => {
            println!("settings.toml is invalid or unreadable; using NNPages defaults.");
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    tempfile::TempPath::try_from_path(temporary.to_path_buf())?
        .persist(destination)
        .map_err(|error| error.error)
}

pub(crate) fn root(config_dir: &Path) -> PathBuf {
    config_dir.join(DIRECTORY_NAME)
}

pub(crate) fn page_root(config_dir: &Path) -> PathBuf {
    root(config_dir).join(PAGES_DIRECTORY_NAME)
}

pub(crate) fn file_root(config_dir: &Path) -> PathBuf {
    root(config_dir).join(FILES_DIRECTORY_NAME)
}

pub(crate) fn settings_path(config_dir: &Path) -> PathBuf {
    root(config_dir).join(SETTINGS_FILE_NAME)
}

pub(crate) fn read_node_name(path: &Path) -> Option<String> {
    let (root, name) = (path.parent()?, path.file_name()?);
    let mut opened = open_hosted(
        root,
        Path::new(name),
        u64::try_from(MAX_ANNOUNCE_APP_DATA_LEN)
            .ok()?
            .saturating_add(2),
    )
    .ok()?;
    let mut text = String::new();
    opened.file.read_to_string(&mut text).ok()?;
    match validate_node_name(&text) {
        Ok(name) => Some(name.to_string()),
        Err(reason) => {
            tracing::warn!(
                event = "nnpages_name_invalid",
                path = %path.display(),
                reason = reason.as_str(),
            );
            None
        }
    }
}

fn prepare_operator_root(root: &Path) -> io::Result<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a directory", root.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir_all(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeNameValidationError {
    Invalid,
    TooLong,
}

impl NodeNameValidationError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "the name must be one non-empty line without control characters",
            Self::TooLong => "the name exceeds announce app-data capacity",
        }
    }
}

fn validate_node_name(value: &str) -> Result<&str, NodeNameValidationError> {
    let name = value.trim();
    if name.is_empty() || name.chars().any(char::is_control) || name.lines().count() != 1 {
        return Err(NodeNameValidationError::Invalid);
    }
    if name.len() > MAX_ANNOUNCE_APP_DATA_LEN {
        return Err(NodeNameValidationError::TooLong);
    }
    Ok(name)
}

pub(crate) fn is_page_available(path: &Path) -> bool {
    let (Some(root), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    if safe_page_name(name).is_none() {
        return false;
    }
    open_hosted(root, Path::new(name), MAX_PAGE_BYTES).is_ok()
}

fn safe_component_name(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    if name.starts_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(name.to_owned())
}

fn safe_page_name(name: &std::ffi::OsStr) -> Option<String> {
    let name = safe_component_name(name)?;
    if !name.ends_with(".mu") {
        return None;
    }
    Some(name)
}

fn scan_routes(pages_root: &Path, files_root: &Path) -> io::Result<Vec<HostedRoute>> {
    let mut routes = Vec::new();
    let mut scanned_entries = 0usize;
    collect_tree(
        pages_root,
        Path::new(""),
        String::new(),
        HostedKind::Page,
        0,
        &mut scanned_entries,
        &mut routes,
    )?;
    collect_tree(
        files_root,
        Path::new(""),
        String::new(),
        HostedKind::File,
        0,
        &mut scanned_entries,
        &mut routes,
    )?;
    routes.sort_by(|left, right| left.request_path.cmp(&right.request_path));
    Ok(routes)
}

fn collect_tree(
    directory: &Path,
    relative_path: &Path,
    relative_name: String,
    kind: HostedKind,
    depth: usize,
    scanned_entries: &mut usize,
    routes: &mut Vec<HostedRoute>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        *scanned_entries = scanned_entries.saturating_add(1);
        if *scanned_entries > MAX_SCAN_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("hosted directory scan exceeds {MAX_SCAN_ENTRIES} entries"),
            ));
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if depth >= MAX_HOSTED_DEPTH {
                tracing::warn!(
                    event = "hosted_route_depth_exceeded",
                    path = %entry.path().display(),
                    maximum_depth = MAX_HOSTED_DEPTH,
                );
                continue;
            }
            let Some(directory_name) = safe_component_name(&entry.file_name()) else {
                continue;
            };
            collect_tree(
                &entry.path(),
                &relative_path.join(&directory_name),
                format!("{relative_name}{directory_name}/"),
                kind,
                depth + 1,
                scanned_entries,
                routes,
            )?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let file_name = match kind {
            HostedKind::Page => safe_page_name(&entry.file_name()),
            HostedKind::File => safe_component_name(&entry.file_name()),
        };
        let Some(file_name) = file_name else {
            continue;
        };
        let served_name = format!("{relative_name}{file_name}");
        if served_name.len() > MAX_SERVED_NAME_BYTES {
            tracing::warn!(
                event = "hosted_route_name_too_long",
                name = served_name,
                maximum_bytes = MAX_SERVED_NAME_BYTES,
            );
            continue;
        }
        let request_path = format!("{}{served_name}", kind.request_prefix());
        if routes.len() >= MAX_HOSTED_ROUTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("hosted route count exceeds {MAX_HOSTED_ROUTES}"),
            ));
        }
        routes.push(HostedRoute {
            path_hash: RequestPathHash::of(&request_path),
            relative_path: relative_path.join(file_name),
            request_path,
            kind,
        });
    }
    Ok(())
}

#[derive(Debug)]
enum HostedReadError {
    Unavailable,
    TooLarge,
    Read(io::Error),
}

struct OpenHosted {
    file: File,
    byte_len: u64,
}

fn classify_open_error(error: io::Error) -> HostedReadError {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::InvalidInput => {
            HostedReadError::Unavailable
        }
        _ => HostedReadError::Read(error),
    }
}

fn validate_opened_file(file: File, max_bytes: u64) -> Result<OpenHosted, HostedReadError> {
    let metadata = file.metadata().map_err(HostedReadError::Read)?;
    if !metadata.file_type().is_file() {
        return Err(HostedReadError::Unavailable);
    }
    if metadata.len() > max_bytes {
        return Err(HostedReadError::TooLarge);
    }
    Ok(OpenHosted {
        file,
        byte_len: metadata.len(),
    })
}

#[cfg(unix)]
fn open_hosted(
    root: &Path,
    relative_path: &Path,
    max_bytes: u64,
) -> Result<OpenHosted, HostedReadError> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let directory_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW;
    let mut directory = openat(CWD, root, directory_flags, Mode::empty())
        .map_err(io::Error::from)
        .map_err(classify_open_error)?;
    let mut components = relative_path.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(component) = component else {
            return Err(HostedReadError::Unavailable);
        };
        if components.peek().is_some() {
            directory = openat(&directory, component, directory_flags, Mode::empty())
                .map_err(io::Error::from)
                .map_err(classify_open_error)?;
            continue;
        }
        let descriptor = openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .map_err(classify_open_error)?;
        return validate_opened_file(File::from(descriptor), max_bytes);
    }
    Err(HostedReadError::Unavailable)
}

#[cfg(not(unix))]
fn open_hosted(
    root: &Path,
    relative_path: &Path,
    max_bytes: u64,
) -> Result<OpenHosted, HostedReadError> {
    let canonical_root = root.canonicalize().map_err(classify_open_error)?;
    let canonical_target = root
        .join(relative_path)
        .canonicalize()
        .map_err(classify_open_error)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(HostedReadError::Unavailable);
    }
    let file = File::open(canonical_target).map_err(classify_open_error)?;
    validate_opened_file(file, max_bytes)
}

pub(crate) fn served_file_len(config_dir: &Path, name: &str) -> Option<u64> {
    if safe_component_name(std::ffi::OsStr::new(name)).as_deref() != Some(name) {
        return None;
    }
    open_hosted(&file_root(config_dir), Path::new(name), MAX_FILE_BYTES)
        .ok()
        .map(|opened| opened.byte_len)
}

#[cfg(test)]
mod tests;
