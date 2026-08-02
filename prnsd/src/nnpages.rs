use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use personal_rns::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::runtime::request_endpoints::{Decline, RequestContext};
use personal_rns::runtime::{PrnsNodeHandle, RuntimeRequestHandlerError};
use personal_rns::wire::DestinationHash;

use crate::services::DaemonRequestState;

mod settings;

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
    include_str!("../../assets/nnpages/masthead.mu"),
    include_str!("../assets/nnpages/index_welcome.mu"),
    include_str!("../../assets/nnpages/nav.mu"),
    include_str!("../../assets/nnpages/why_prns.mu"),
    include_str!("../../assets/nnpages/license.mu"),
    include_str!("../../assets/nnpages/quote.mu"),
    include_str!("../assets/nnpages/index_outro.mu"),
    include_str!("../../assets/nnpages/credits.mu"),
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
const CONTROL_DIRECTORY_NAME: &str = ".prnsd-control/nnpages";
const CONTROL_VERSION: &str = "prnsd-nnpages-refresh-v1";
const CONTROL_ANNOUNCE_VERSION: &str = "prnsd-nnpages-announce-v1";
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

static CONTROL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    RefreshFailed,
    AnnounceFailed,
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
            Self::TimedOut => {
                formatter.write_str("the daemon did not acknowledge the request within 10 seconds")
            }
            Self::RefreshFailed => {
                formatter.write_str("the daemon could not refresh its NNPages catalog")
            }
            Self::AnnounceFailed => {
                formatter.write_str("the daemon could not announce the page destination")
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
            | Self::RefreshFailed
            | Self::AnnounceFailed
            | Self::InvalidName
            | Self::NameTooLong
            | Self::InvalidResult => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NnPagesControlKind {
    Refresh,
    Announce,
}

pub(crate) struct PendingNnPagesRefresh {
    id: u128,
    kind: NnPagesControlKind,
    request_path: PathBuf,
    result_path: PathBuf,
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
                let action = if staged.created.is_empty() {
                    "Verified"
                } else {
                    "Staged"
                };
                println!(
                    "{action} {} ({}); checksum available at files/{SOURCE_CHECKSUM_FILE_NAME}.",
                    staged.archive_path.display(),
                    format_archive_size(staged.archive_bytes),
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

pub(crate) async fn next_refresh_request(config_dir: &Path) -> io::Result<PendingNnPagesRefresh> {
    let control = control_root(config_dir);
    loop {
        tokio::time::sleep(CONTROL_POLL_INTERVAL).await;
        let entries = match fs::read_dir(&control) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut requests = entries
            .filter_map(Result::ok)
            .filter_map(|entry| decode_request(&entry.path()))
            .collect::<Vec<_>>();
        requests.sort_by_key(|request| request.id);
        if let Some(request) = requests.into_iter().next() {
            return Ok(request);
        }
    }
}

impl PendingNnPagesRefresh {
    pub(crate) fn kind(&self) -> NnPagesControlKind {
        self.kind
    }

    pub(crate) fn finish(
        self,
        result: Result<NnPagesRefreshReport, NnPagesRefreshError>,
    ) -> io::Result<()> {
        let encoded = encode_control_result(self.id, result.ok().as_ref());
        atomic_control_write(&self.result_path, encoded.as_bytes())?;
        match fs::remove_file(&self.request_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn finish_announce(self, succeeded: bool) -> io::Result<()> {
        let encoded = encode_announce_result(self.id, succeeded);
        atomic_control_write(&self.result_path, encoded.as_bytes())?;
        match fs::remove_file(&self.request_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

async fn request_refresh(config_dir: &Path) -> Result<NnPagesRefreshReport, NnPagesCliError> {
    let control = control_root(config_dir);
    fs::create_dir_all(&control).map_err(NnPagesCliError::Control)?;
    let id = next_control_id();
    let request_path = control.join(format!("request-{id:032x}"));
    let result_path = control.join(format!("result-{id:032x}"));
    let request = format!("{CONTROL_VERSION}\n{id:032x}\n");
    create_control_request(&request_path, request.as_bytes()).map_err(NnPagesCliError::Control)?;

    let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
    loop {
        match fs::read_to_string(&result_path) {
            Ok(text) => {
                let _ = fs::remove_file(&result_path);
                let _ = fs::remove_file(&request_path);
                return decode_control_result(id, &text);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_file(&request_path);
                return Err(NnPagesCliError::Control(error));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = fs::remove_file(&request_path);
            return Err(NnPagesCliError::TimedOut);
        }
        tokio::time::sleep(CONTROL_POLL_INTERVAL).await;
    }
}

fn control_root(config_dir: &Path) -> PathBuf {
    config_dir.join(CONTROL_DIRECTORY_NAME)
}

fn next_control_id() -> u128 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let process = u128::from(std::process::id()) << 64;
    let sequence = u128::from(CONTROL_SEQUENCE.fetch_add(1, Ordering::Relaxed));
    time ^ process ^ sequence
}

fn create_control_request(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn atomic_control_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        CONTROL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    create_control_request(&temporary, bytes)?;
    match replace_file(&temporary, path) {
        Ok(()) => sync_parent_directory(path),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic target has no parent directory",
        )
    })?;
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
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

fn decode_request(path: &Path) -> Option<PendingNnPagesRefresh> {
    let file_name = path.file_name()?.to_str()?;
    let id = u128::from_str_radix(file_name.strip_prefix("request-")?, 16).ok()?;
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let kind = match lines.next()? {
        CONTROL_VERSION => NnPagesControlKind::Refresh,
        CONTROL_ANNOUNCE_VERSION => NnPagesControlKind::Announce,
        _ => return None,
    };
    if u128::from_str_radix(lines.next()?, 16).ok()? != id || lines.next().is_some() {
        return None;
    }
    Some(PendingNnPagesRefresh {
        id,
        kind,
        request_path: path.to_path_buf(),
        result_path: path.with_file_name(format!("result-{id:032x}")),
    })
}

async fn request_announce(config_dir: &Path) -> Result<(), NnPagesCliError> {
    let control = control_root(config_dir);
    fs::create_dir_all(&control).map_err(NnPagesCliError::Control)?;
    let id = next_control_id();
    let request_path = control.join(format!("request-{id:032x}"));
    let result_path = control.join(format!("result-{id:032x}"));
    let request = format!("{CONTROL_ANNOUNCE_VERSION}\n{id:032x}\n");
    create_control_request(&request_path, request.as_bytes()).map_err(NnPagesCliError::Control)?;

    let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
    loop {
        match fs::read_to_string(&result_path) {
            Ok(text) => {
                let _ = fs::remove_file(&result_path);
                let _ = fs::remove_file(&request_path);
                return decode_announce_result(id, &text);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_file(&request_path);
                return Err(NnPagesCliError::Control(error));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = fs::remove_file(&request_path);
            return Err(NnPagesCliError::TimedOut);
        }
        tokio::time::sleep(CONTROL_POLL_INTERVAL).await;
    }
}

fn encode_announce_result(id: u128, succeeded: bool) -> String {
    match succeeded {
        true => format!("{CONTROL_ANNOUNCE_VERSION}\n{id:032x}\nok\n"),
        false => format!("{CONTROL_ANNOUNCE_VERSION}\n{id:032x}\nfailed\n"),
    }
}

fn decode_announce_result(id: u128, text: &str) -> Result<(), NnPagesCliError> {
    let mut lines = text.lines();
    if lines.next() != Some(CONTROL_ANNOUNCE_VERSION)
        || lines
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            != Some(id)
    {
        return Err(NnPagesCliError::InvalidResult);
    }
    match (lines.next(), lines.next()) {
        (Some("ok"), None) => Ok(()),
        (Some("failed"), None) => Err(NnPagesCliError::AnnounceFailed),
        _ => Err(NnPagesCliError::InvalidResult),
    }
}

fn encode_control_result(id: u128, report: Option<&NnPagesRefreshReport>) -> String {
    match report {
        Some(report) => format!(
            "{CONTROL_VERSION}\n{id:032x}\nok\n{}\n{}\n{}\n{}\n{}\n{}\n",
            report.discovered,
            report.added,
            report.removed,
            report.unchanged,
            report.settings_status.as_control_value(),
            if report.settings_changed {
                "changed"
            } else {
                "unchanged"
            },
        ),
        None => format!("{CONTROL_VERSION}\n{id:032x}\nfailed\n"),
    }
}

fn decode_control_result(id: u128, text: &str) -> Result<NnPagesRefreshReport, NnPagesCliError> {
    let mut lines = text.lines();
    if lines.next() != Some(CONTROL_VERSION)
        || lines
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            != Some(id)
    {
        return Err(NnPagesCliError::InvalidResult);
    }
    match lines.next() {
        Some("failed") if lines.next().is_none() => Err(NnPagesCliError::RefreshFailed),
        Some("ok") => {
            let report = NnPagesRefreshReport {
                discovered: parse_control_count(lines.next())?,
                added: parse_control_count(lines.next())?,
                removed: parse_control_count(lines.next())?,
                unchanged: parse_control_count(lines.next())?,
                settings_status: lines
                    .next()
                    .and_then(NnPagesSettingsStatus::from_control_value)
                    .ok_or(NnPagesCliError::InvalidResult)?,
                settings_changed: match lines.next() {
                    Some("changed") => true,
                    Some("unchanged") => false,
                    _ => return Err(NnPagesCliError::InvalidResult),
                },
            };
            if lines.next().is_some() {
                return Err(NnPagesCliError::InvalidResult);
            }
            Ok(report)
        }
        _ => Err(NnPagesCliError::InvalidResult),
    }
}

fn parse_control_count(value: Option<&str>) -> Result<usize, NnPagesCliError> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or(NnPagesCliError::InvalidResult)
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
mod tests {
    use super::*;
    use personal_rns::engine::RatchetPolicy;

    #[test]
    fn operator_layout_is_isolated_beneath_nnpages() {
        let config = Path::new("/var/lib/prnsd");
        assert_eq!(root(config), config.join("nnpages"));
        assert_eq!(page_root(config), config.join("nnpages/pages"));
        assert_eq!(file_root(config), config.join("nnpages/files"));
        assert_eq!(settings_path(config), config.join("nnpages/settings.toml"));
        assert_eq!(
            NnPagesCatalog::empty(config).node_name_path(),
            config.join("nnpages/name")
        );
    }

    #[test]
    fn settings_creation_alone_requires_a_live_refresh() {
        assert!(seed_requires_refresh(true, false, false, false));
        assert!(!seed_requires_refresh(false, false, false, false));
    }

    #[test]
    fn the_node_name_file_is_never_published() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir_all(&root).expect("page root");
        fs::write(root.join(INDEX_FILE_NAME), b"index").expect("index");
        fs::write(root.join(NODE_NAME_FILE_NAME), b"Frosty Relay").expect("name");
        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
        assert_eq!(
            catalog.request_paths(),
            vec![String::from("/page/index.mu")]
        );
        assert_eq!(
            safe_page_name(std::ffi::OsStr::new(NODE_NAME_FILE_NAME)),
            None
        );
    }

    #[test]
    fn node_names_read_trimmed_and_blank_is_none() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(NODE_NAME_FILE_NAME);
        fs::write(&path, "  Frosty Relay \n").expect("name");
        assert_eq!(read_node_name(&path).as_deref(), Some("Frosty Relay"));
        fs::write(&path, " \n").expect("blank");
        assert_eq!(read_node_name(&path), None);
        fs::write(&path, "first\nsecond").expect("multiline");
        assert_eq!(read_node_name(&path), None);
        fs::write(&path, "control\tname").expect("control");
        assert_eq!(read_node_name(&path), None);
        fs::write(&path, "x".repeat(MAX_ANNOUNCE_APP_DATA_LEN + 1)).expect("long");
        assert_eq!(read_node_name(&path), None);
        assert_eq!(read_node_name(&directory.path().join("absent")), None);
    }

    #[test]
    fn node_name_writes_atomically_replace_complete_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = super::root(directory.path());
        prepare_operator_root(&root).expect("NNPages root");
        let path = root.join(NODE_NAME_FILE_NAME);

        atomic_control_write(&path, b"First Name").expect("first name");
        atomic_control_write(&path, b"Replacement Name").expect("replacement name");

        assert_eq!(
            fs::read_to_string(path).expect("name is readable"),
            "Replacement Name"
        );
    }

    #[tokio::test]
    async fn rename_succeeds_durably_when_index_is_unavailable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        run_cli(crate::cli::NnPagesArgs {
            command: crate::cli::NnPagesCommand::Rename(crate::cli::NnPagesRenameArgs {
                name: String::from("My Node"),
                config: Some(directory.path().to_path_buf()),
            }),
        })
        .await
        .expect("rename succeeds");
        assert_eq!(
            fs::read_to_string(root(directory.path()).join(NODE_NAME_FILE_NAME))
                .expect("saved name"),
            "My Node"
        );
    }

    #[test]
    fn announce_control_results_round_trip() {
        let id = 7u128;
        assert!(matches!(
            decode_announce_result(id, &encode_announce_result(id, true)),
            Ok(())
        ));
        assert!(matches!(
            decode_announce_result(id, &encode_announce_result(id, false)),
            Err(NnPagesCliError::AnnounceFailed)
        ));
    }

    #[test]
    fn announce_requests_decode_with_their_own_version() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let id = 0x2au128;
        let path = directory.path().join(format!("request-{id:032x}"));
        fs::write(&path, format!("{CONTROL_ANNOUNCE_VERSION}\n{id:032x}\n")).expect("request");
        let request = decode_request(&path).expect("decodes");
        assert_eq!(request.kind(), NnPagesControlKind::Announce);
    }
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
        ServeMyRequestEndpoints,
    };
    use personal_rns::storage::GrowableHeap;

    #[test]
    fn catalog_indexes_safe_mu_files_and_recurses_into_safe_directories() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir_all(&root).expect("page root");
        fs::write(root.join("index.mu"), b"index").expect("index");
        fs::write(root.join("about-us_2.mu"), b"about").expect("about");
        fs::write(root.join("ignored.txt"), b"ignored").expect("ignored");
        fs::write(root.join(".private.mu"), b"private").expect("private");
        fs::create_dir(root.join("docs")).expect("docs directory");
        fs::write(root.join("docs/guide.mu"), b"guide").expect("guide");
        fs::create_dir(root.join("docs/deep")).expect("deep directory");
        fs::write(root.join("docs/deep/detail.mu"), b"detail").expect("detail");
        fs::create_dir(root.join(".hidden")).expect("hidden directory");
        fs::write(root.join(".hidden/secret.mu"), b"secret").expect("secret");
        fs::create_dir(root.join("nested.mu")).expect("nested directory");

        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");

        assert_eq!(
            catalog.request_paths(),
            [
                "/page/about-us_2.mu",
                "/page/docs/deep/detail.mu",
                "/page/docs/guide.mu",
                "/page/index.mu"
            ]
        );
    }

    #[test]
    fn the_files_directory_serves_safe_names_under_file_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let pages = page_root(directory.path());
        fs::create_dir_all(&pages).expect("page root");
        fs::write(pages.join(INDEX_FILE_NAME), b"index").expect("index");
        let files = file_root(directory.path());
        fs::create_dir(&files).expect("file root");
        fs::write(files.join("demo.txt"), b"demo").expect("demo");
        fs::write(files.join("download.mu"), b"download").expect("mu download");
        fs::create_dir(files.join("sub")).expect("sub directory");
        fs::write(files.join("sub/data.bin"), b"data").expect("data");
        fs::write(files.join(".hidden"), b"hidden").expect("hidden");

        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");

        assert_eq!(
            catalog.request_paths(),
            [
                "/file/demo.txt",
                "/file/download.mu",
                "/file/sub/data.bin",
                "/page/index.mu"
            ]
        );
    }

    #[test]
    fn page_bytes_are_read_fresh_and_deletion_is_unavailable() {
        use std::io::Read;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let path = root.join(INDEX_FILE_NAME);
        fs::write(&path, b"first").expect("first page");
        let mut first = open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES)
            .expect("first open")
            .file;
        let mut first_bytes = Vec::new();
        first.read_to_end(&mut first_bytes).expect("first read");
        assert_eq!(first_bytes, b"first");

        fs::write(&path, b"second").expect("second page");
        let mut second = open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES)
            .expect("second open")
            .file;
        let mut second_bytes = Vec::new();
        second.read_to_end(&mut second_bytes).expect("second read");
        assert_eq!(second_bytes, b"second");

        fs::remove_file(&path).expect("delete page");
        assert!(matches!(
            open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES),
            Err(HostedReadError::Unavailable)
        ));
    }

    #[test]
    fn hosted_reads_enforce_the_kinds_size_limit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("data.bin");
        fs::write(&path, b"12345").expect("data");
        assert_eq!(
            open_hosted(directory.path(), Path::new("data.bin"), 5)
                .expect("fits")
                .byte_len,
            5
        );
        assert!(matches!(
            open_hosted(directory.path(), Path::new("data.bin"), 4),
            Err(HostedReadError::TooLarge)
        ));
    }

    #[tokio::test]
    async fn live_refresh_registers_added_paths_and_retires_removed_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir_all(&root).expect("page root");
        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
        let mut node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let destination = node
            .register_preconfigured_destination(PreConfiguredDestination::Single {
                app_name: "nomadnetwork",
                aspects: &["node"],
                identity: Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]),
                announce_app_data: &[],
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                request_endpoints: ServeMyRequestEndpoints::No,
            })
            .expect("destination");
        let handle = node.handle();
        let mut announcement_settings = catalog.announcement_settings();
        let exercise = async {
            fs::write(root.join("index.mu"), b"index").expect("index");
            fs::write(root.join("about.mu"), b"about").expect("about");
            let files = file_root(directory.path());
            fs::create_dir(&files).expect("file root");
            fs::write(files.join("hello.txt"), b"hello").expect("hello");
            let added = catalog
                .refresh(&handle, destination)
                .await
                .expect("add routes");
            assert_eq!(
                added,
                NnPagesRefreshReport {
                    discovered: 3,
                    added: 3,
                    removed: 0,
                    unchanged: 0,
                    settings_status: NnPagesSettingsStatus::MissingDefaults,
                    settings_changed: false,
                }
            );
            assert_eq!(
                catalog.request_paths(),
                ["/file/hello.txt", "/page/about.mu", "/page/index.mu"]
            );

            fs::remove_file(root.join("about.mu")).expect("remove about");
            fs::write(
                settings_path(directory.path()),
                "announce = false\nannounce_interval_minutes = 45\n",
            )
            .expect("changed settings");
            let removed = catalog
                .refresh(&handle, destination)
                .await
                .expect("remove route");
            assert_eq!(
                removed,
                NnPagesRefreshReport {
                    discovered: 2,
                    added: 0,
                    removed: 1,
                    unchanged: 2,
                    settings_status: NnPagesSettingsStatus::Loaded,
                    settings_changed: true,
                }
            );
            announcement_settings
                .changed()
                .await
                .expect("settings update");
            assert!(!announcement_settings.borrow_and_update().announce());
            assert_eq!(
                catalog.request_paths(),
                ["/file/hello.txt", "/page/index.mu"]
            );

            let unchanged = catalog
                .refresh(&handle, destination)
                .await
                .expect("unchanged refresh");
            assert_eq!(unchanged.settings_status, NnPagesSettingsStatus::Loaded);
            assert!(!unchanged.settings_changed);
        };
        tokio::pin!(exercise);
        tokio::select! {
            () = &mut exercise => {}
            result = node.run() => panic!("node stopped during refresh: {result:?}"),
        }
    }

    #[tokio::test]
    async fn config_local_refresh_control_returns_the_daemon_report() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_refresh(&client_dir).await });
        let pending =
            tokio::time::timeout(Duration::from_secs(2), next_refresh_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");
        let report = NnPagesRefreshReport {
            discovered: 3,
            added: 1,
            removed: 1,
            unchanged: 2,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: true,
        };
        pending.finish(Ok(report)).expect("result written");
        assert_eq!(
            client.await.expect("client joins").expect("refresh"),
            report
        );
    }

    #[test]
    fn control_results_reject_wrong_identity_and_failure_is_typed() {
        let report = NnPagesRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
            settings_status: NnPagesSettingsStatus::InvalidDefaults,
            settings_changed: false,
        };
        assert_eq!(
            decode_control_result(7, &encode_control_result(7, Some(&report)))
                .expect("valid report"),
            report
        );
        assert!(matches!(
            decode_control_result(8, &encode_control_result(7, Some(&report))),
            Err(NnPagesCliError::InvalidResult)
        ));
        assert!(matches!(
            decode_control_result(7, &encode_control_result(7, None)),
            Err(NnPagesCliError::RefreshFailed)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_published_served_or_traversed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir_all(&root).expect("page root");
        let source = directory.path().join("source.mu");
        fs::write(&source, b"outside").expect("source");
        let linked = root.join("linked.mu");
        symlink(&source, &linked).expect("symlink");
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).expect("outside directory");
        fs::write(outside.join("leak.mu"), b"leak").expect("leak");
        symlink(&outside, root.join("tour")).expect("directory symlink");

        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
        assert!(catalog.request_paths().is_empty());
        assert!(matches!(
            open_hosted(&root, Path::new("linked.mu"), MAX_PAGE_BYTES),
            Err(HostedReadError::Unavailable) | Err(HostedReadError::Read(_))
        ));

        let external_name = directory.path().join("external-name");
        fs::write(&external_name, b"Outside Name").expect("external name");
        let name = super::root(directory.path()).join(NODE_NAME_FILE_NAME);
        symlink(&external_name, &name).expect("name symlink");
        assert_eq!(read_node_name(&name), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_replaced_by_a_symlink_after_scan_cannot_escape_the_root() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        let section = root.join("section");
        fs::create_dir_all(&section).expect("section");
        fs::write(section.join("entry.mu"), b"inside").expect("inside");
        let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
        assert_eq!(catalog.request_paths(), ["/page/section/entry.mu"]);

        fs::remove_file(section.join("entry.mu")).expect("remove entry");
        fs::remove_dir(&section).expect("remove section");
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("entry.mu"), b"outside").expect("outside entry");
        symlink(&outside, &section).expect("replace section with symlink");

        assert!(matches!(
            open_hosted(&root, Path::new("section/entry.mu"), MAX_PAGE_BYTES),
            Err(HostedReadError::Unavailable) | Err(HostedReadError::Read(_))
        ));
    }
}
