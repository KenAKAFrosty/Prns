use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::runtime::request_endpoints::{Decline, RequestContext};
use personal_rns::runtime::{PrnsNodeHandle, RuntimeRequestHandlerError};
use personal_rns::wire::DestinationHash;

use crate::services::DaemonRequestState;

pub(crate) const DIRECTORY_NAME: &str = "pages";
pub(crate) const INDEX_FILE_NAME: &str = "index.mu";
pub(crate) const DEFAULT_INDEX_PAGE: &[u8] = include_bytes!("../assets/pages/index.mu");

const REQUEST_PREFIX: &str = "/page/";
const MAX_PAGE_BYTES: u64 = 1024 * 1024;
const CONTROL_DIRECTORY_NAME: &str = ".prnsd-control/pages";
const CONTROL_VERSION: &str = "prnsd-page-refresh-v1";
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

static CONTROL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct NodePageCatalog {
    root: Arc<PathBuf>,
    pages: Arc<RwLock<Arc<Vec<PublishedPage>>>>,
    reconciliation: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedPage {
    request_path: String,
    path_hash: RequestPathHash,
    disk_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodePageRefreshReport {
    pub(crate) discovered: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) unchanged: usize,
}

#[derive(Debug)]
pub(crate) enum NodePageRefreshError {
    Scan(io::Error),
    Runtime(RuntimeRequestHandlerError),
    CatalogPoisoned,
    DestinationUnavailable,
}

#[derive(Debug)]
pub(crate) enum NodePageCliError {
    Configuration(prns_config::DiscoveryError),
    Control(io::Error),
    TimedOut,
    RefreshFailed,
    InvalidResult,
}

impl core::fmt::Display for NodePageCliError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Configuration(error) => write!(formatter, "config discovery failed: {error}"),
            Self::Control(error) => write!(formatter, "page refresh control failed: {error}"),
            Self::TimedOut => formatter
                .write_str("the daemon did not acknowledge the page refresh within 10 seconds"),
            Self::RefreshFailed => formatter.write_str("the daemon could not refresh its pages"),
            Self::InvalidResult => formatter.write_str("the daemon returned an invalid result"),
        }
    }
}

impl std::error::Error for NodePageCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Control(error) => Some(error),
            Self::TimedOut | Self::RefreshFailed | Self::InvalidResult => None,
        }
    }
}

pub(crate) struct PendingPageRefresh {
    id: u128,
    request_path: PathBuf,
    result_path: PathBuf,
}

impl core::fmt::Display for NodePageRefreshError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Scan(error) => write!(formatter, "could not scan the page directory: {error}"),
            Self::Runtime(error) => {
                write!(
                    formatter,
                    "could not update the node request routes: {error}"
                )
            }
            Self::CatalogPoisoned => formatter.write_str("the page catalog lock was poisoned"),
            Self::DestinationUnavailable => {
                formatter.write_str("this daemon does not own the hosted page destination")
            }
        }
    }
}

impl std::error::Error for NodePageRefreshError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scan(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::CatalogPoisoned | Self::DestinationUnavailable => None,
        }
    }
}

impl NodePageCatalog {
    pub(crate) fn discover(config_dir: &Path) -> io::Result<Self> {
        let root = page_root(config_dir);
        let pages = scan_pages(&root)?;
        Ok(Self {
            root: Arc::new(root),
            pages: Arc::new(RwLock::new(Arc::new(pages))),
            reconciliation: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub(crate) fn empty(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            pages: Arc::new(RwLock::new(Arc::new(Vec::new()))),
            reconciliation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn request_paths(&self) -> Vec<String> {
        self.snapshot()
            .map(|pages| pages.iter().map(|page| page.request_path.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE_NAME)
    }

    pub(crate) async fn refresh(
        &self,
        handle: &PrnsNodeHandle,
        destination: DestinationHash,
    ) -> Result<NodePageRefreshReport, NodePageRefreshError> {
        let _guard = self.reconciliation.lock().await;
        let root = Arc::clone(&self.root);
        let discovered = tokio::task::spawn_blocking(move || scan_pages(root.as_ref()))
            .await
            .map_err(|error| {
                NodePageRefreshError::Scan(io::Error::other(format!(
                    "page scanner task failed: {error}"
                )))
            })?
            .map_err(NodePageRefreshError::Scan)?;
        let current = self
            .snapshot()
            .ok_or(NodePageRefreshError::CatalogPoisoned)?;
        let added = discovered
            .iter()
            .filter(|candidate| {
                !current
                    .iter()
                    .any(|page| page.request_path == candidate.request_path)
            })
            .cloned()
            .collect::<Vec<_>>();
        let removed = current
            .iter()
            .filter(|candidate| {
                !discovered
                    .iter()
                    .any(|page| page.request_path == candidate.request_path)
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut registered = Vec::new();
        for page in &added {
            if let Err(error) = handle
                .register_request_path(destination, &page.request_path, RequestPolicy::AllowAll)
                .await
            {
                for rollback in registered {
                    let _ = handle.unregister_request_path(destination, rollback).await;
                }
                return Err(NodePageRefreshError::Runtime(error));
            }
            registered.push(page.request_path.as_str());
        }

        {
            let mut pages = self
                .pages
                .write()
                .map_err(|_| NodePageRefreshError::CatalogPoisoned)?;
            *pages = Arc::new(discovered);
        }

        for page in &removed {
            handle
                .unregister_request_path(destination, &page.request_path)
                .await
                .map_err(NodePageRefreshError::Runtime)?;
        }
        let discovered = self
            .snapshot()
            .ok_or(NodePageRefreshError::CatalogPoisoned)?
            .len();
        Ok(NodePageRefreshReport {
            discovered,
            added: added.len(),
            removed: removed.len(),
            unchanged: discovered.saturating_sub(added.len()),
        })
    }

    pub(crate) async fn respond(
        &self,
        mut context: RequestContext<'_, DaemonRequestState>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        let pages = self.snapshot().ok_or(Decline::Ignore)?;
        let Some(page) = pages.iter().find(|page| page.path_hash == path_hash) else {
            return Err(Decline::Ignore);
        };
        let disk_path = page.disk_path.clone();
        let request_path = page.request_path.clone();
        match tokio::task::spawn_blocking(move || read_page(&disk_path)).await {
            Ok(Ok(bytes)) => context.respond_bytes(&bytes),
            Ok(Err(PageReadError::Unavailable)) => Err(Decline::Ignore),
            Ok(Err(PageReadError::TooLarge)) => {
                tracing::warn!(
                    event = "node_page_too_large",
                    path = request_path,
                    maximum_bytes = MAX_PAGE_BYTES,
                );
                Err(Decline::Ignore)
            }
            Ok(Err(PageReadError::Read(error))) => {
                tracing::warn!(
                    event = "node_page_read_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
            Err(error) => {
                tracing::warn!(
                    event = "node_page_reader_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
        }
    }

    fn snapshot(&self) -> Option<Arc<Vec<PublishedPage>>> {
        self.pages.read().ok().map(|pages| Arc::clone(&pages))
    }
}

pub(crate) async fn run_cli(args: crate::cli::PagesArgs) -> Result<(), NodePageCliError> {
    match args.command {
        crate::cli::PagesCommand::Refresh(args) => {
            let discovered = prns_config::discover(args.config.as_deref())
                .map_err(NodePageCliError::Configuration)?;
            let report = request_refresh(&discovered.dir).await?;
            println!(
                "Refreshed {} page route(s): {} added, {} removed, {} unchanged.",
                report.discovered, report.added, report.removed, report.unchanged,
            );
            Ok(())
        }
    }
}

pub(crate) async fn next_refresh_request(config_dir: &Path) -> io::Result<PendingPageRefresh> {
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

impl PendingPageRefresh {
    pub(crate) fn finish(
        self,
        result: Result<NodePageRefreshReport, NodePageRefreshError>,
    ) -> io::Result<()> {
        let encoded = encode_control_result(self.id, result.ok().as_ref());
        atomic_control_write(&self.result_path, encoded.as_bytes())?;
        match fs::remove_file(&self.request_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

async fn request_refresh(config_dir: &Path) -> Result<NodePageRefreshReport, NodePageCliError> {
    let control = control_root(config_dir);
    fs::create_dir_all(&control).map_err(NodePageCliError::Control)?;
    let id = next_control_id();
    let request_path = control.join(format!("request-{id:032x}"));
    let result_path = control.join(format!("result-{id:032x}"));
    let request = format!("{CONTROL_VERSION}\n{id:032x}\n");
    create_control_request(&request_path, request.as_bytes()).map_err(NodePageCliError::Control)?;

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
                return Err(NodePageCliError::Control(error));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = fs::remove_file(&request_path);
            return Err(NodePageCliError::TimedOut);
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
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn decode_request(path: &Path) -> Option<PendingPageRefresh> {
    let file_name = path.file_name()?.to_str()?;
    let id = u128::from_str_radix(file_name.strip_prefix("request-")?, 16).ok()?;
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != CONTROL_VERSION
        || u128::from_str_radix(lines.next()?, 16).ok()? != id
        || lines.next().is_some()
    {
        return None;
    }
    Some(PendingPageRefresh {
        id,
        request_path: path.to_path_buf(),
        result_path: path.with_file_name(format!("result-{id:032x}")),
    })
}

fn encode_control_result(id: u128, report: Option<&NodePageRefreshReport>) -> String {
    match report {
        Some(report) => format!(
            "{CONTROL_VERSION}\n{id:032x}\nok\n{}\n{}\n{}\n{}\n",
            report.discovered, report.added, report.removed, report.unchanged,
        ),
        None => format!("{CONTROL_VERSION}\n{id:032x}\nfailed\n"),
    }
}

fn decode_control_result(id: u128, text: &str) -> Result<NodePageRefreshReport, NodePageCliError> {
    let mut lines = text.lines();
    if lines.next() != Some(CONTROL_VERSION)
        || lines
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            != Some(id)
    {
        return Err(NodePageCliError::InvalidResult);
    }
    match lines.next() {
        Some("failed") if lines.next().is_none() => Err(NodePageCliError::RefreshFailed),
        Some("ok") => {
            let report = NodePageRefreshReport {
                discovered: parse_control_count(lines.next())?,
                added: parse_control_count(lines.next())?,
                removed: parse_control_count(lines.next())?,
                unchanged: parse_control_count(lines.next())?,
            };
            if lines.next().is_some() {
                return Err(NodePageCliError::InvalidResult);
            }
            Ok(report)
        }
        _ => Err(NodePageCliError::InvalidResult),
    }
}

fn parse_control_count(value: Option<&str>) -> Result<usize, NodePageCliError> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or(NodePageCliError::InvalidResult)
}

pub(crate) fn page_root(config_dir: &Path) -> PathBuf {
    config_dir.join(DIRECTORY_NAME)
}

pub(crate) fn is_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn safe_page_name(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    if name.starts_with('.')
        || !name.ends_with(".mu")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(name.to_owned())
}

fn scan_pages(root: &Path) -> io::Result<Vec<PublishedPage>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut pages = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(file_name) = safe_page_name(&entry.file_name()) else {
            continue;
        };
        let request_path = format!("{REQUEST_PREFIX}{file_name}");
        pages.push(PublishedPage {
            path_hash: RequestPathHash::of(&request_path),
            disk_path: entry.path(),
            request_path,
        });
    }
    pages.sort_by(|left, right| left.request_path.cmp(&right.request_path));
    Ok(pages)
}

#[derive(Debug)]
enum PageReadError {
    Unavailable,
    TooLarge,
    Read(io::Error),
}

fn read_page(path: &Path) -> Result<Vec<u8>, PageReadError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PageReadError::Unavailable
        } else {
            PageReadError::Read(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(PageReadError::Unavailable);
    }
    if metadata.len() > MAX_PAGE_BYTES {
        return Err(PageReadError::TooLarge);
    }
    let file = File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PageReadError::Unavailable
        } else {
            PageReadError::Read(error)
        }
    })?;
    let mut bytes =
        Vec::with_capacity(usize::try_from(metadata.len().min(MAX_PAGE_BYTES)).unwrap_or_default());
    file.take(MAX_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(PageReadError::Read)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PAGE_BYTES {
        return Err(PageReadError::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::engine::RatchetPolicy;
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
        ServeMyRequestEndpoints,
    };
    use personal_rns::storage::GrowableHeap;

    #[test]
    fn catalog_indexes_only_safe_top_level_mu_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir(&root).expect("page root");
        fs::write(root.join("index.mu"), b"index").expect("index");
        fs::write(root.join("about-us_2.mu"), b"about").expect("about");
        fs::write(root.join("ignored.txt"), b"ignored").expect("ignored");
        fs::write(root.join(".private.mu"), b"private").expect("private");
        fs::create_dir(root.join("nested.mu")).expect("nested directory");

        let catalog = NodePageCatalog::discover(directory.path()).expect("catalog");

        assert_eq!(
            catalog.request_paths(),
            ["/page/about-us_2.mu", "/page/index.mu"]
        );
    }

    #[test]
    fn page_bytes_are_read_fresh_and_deletion_is_unavailable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(INDEX_FILE_NAME);
        fs::write(&path, b"first").expect("first page");
        assert_eq!(read_page(&path).expect("first read"), b"first");

        fs::write(&path, b"second").expect("second page");
        assert_eq!(read_page(&path).expect("second read"), b"second");

        fs::remove_file(&path).expect("delete page");
        assert!(matches!(read_page(&path), Err(PageReadError::Unavailable)));
    }

    #[tokio::test]
    async fn live_refresh_registers_added_paths_and_retires_removed_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir(&root).expect("page root");
        let catalog = NodePageCatalog::discover(directory.path()).expect("catalog");
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
        let exercise = async {
            fs::write(root.join("index.mu"), b"index").expect("index");
            fs::write(root.join("about.mu"), b"about").expect("about");
            let added = catalog
                .refresh(&handle, destination)
                .await
                .expect("add routes");
            assert_eq!(
                added,
                NodePageRefreshReport {
                    discovered: 2,
                    added: 2,
                    removed: 0,
                    unchanged: 0,
                }
            );
            assert_eq!(
                catalog.request_paths(),
                ["/page/about.mu", "/page/index.mu"]
            );

            fs::remove_file(root.join("about.mu")).expect("remove about");
            let removed = catalog
                .refresh(&handle, destination)
                .await
                .expect("remove route");
            assert_eq!(
                removed,
                NodePageRefreshReport {
                    discovered: 1,
                    added: 0,
                    removed: 1,
                    unchanged: 1,
                }
            );
            assert_eq!(catalog.request_paths(), ["/page/index.mu"]);
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
        let report = NodePageRefreshReport {
            discovered: 3,
            added: 1,
            removed: 1,
            unchanged: 2,
        };
        pending.finish(Ok(report)).expect("result written");
        assert_eq!(
            client.await.expect("client joins").expect("refresh"),
            report
        );
    }

    #[test]
    fn control_results_reject_wrong_identity_and_failure_is_typed() {
        let report = NodePageRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
        };
        assert_eq!(
            decode_control_result(7, &encode_control_result(7, Some(&report)))
                .expect("valid report"),
            report
        );
        assert!(matches!(
            decode_control_result(8, &encode_control_result(7, Some(&report))),
            Err(NodePageCliError::InvalidResult)
        ));
        assert!(matches!(
            decode_control_result(7, &encode_control_result(7, None)),
            Err(NodePageCliError::RefreshFailed)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_published_or_served() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir(&root).expect("page root");
        let source = directory.path().join("source.mu");
        fs::write(&source, b"outside").expect("source");
        let linked = root.join("linked.mu");
        symlink(&source, &linked).expect("symlink");

        let catalog = NodePageCatalog::discover(directory.path()).expect("catalog");
        assert!(catalog.request_paths().is_empty());
        assert!(matches!(
            read_page(&linked),
            Err(PageReadError::Unavailable)
        ));
    }
}
