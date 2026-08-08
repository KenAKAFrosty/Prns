use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::replace_file;
use super::{NnPagesCliError, NnPagesRefreshError, NnPagesRefreshReport, NnPagesSettingsStatus};

const CONTROL_DIRECTORY_NAME: &str = ".prnsd-control/nnpages";
const CONTROL_REFRESH_VERSION: &str = "prnsd-nnpages-refresh-v2";
const CONTROL_ANNOUNCE_VERSION: &str = "prnsd-nnpages-announce-v2";
const CONTROL_REQUEST_PREFIX: &str = "request-";
const CONTROL_INFLIGHT_PREFIX: &str = "inflight-";
const CONTROL_RESULT_PREFIX: &str = "result-";
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

static CONTROL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NnPagesControlKind {
    Refresh,
    Announce,
}

impl NnPagesControlKind {
    const fn version(self) -> &'static str {
        match self {
            Self::Refresh => CONTROL_REFRESH_VERSION,
            Self::Announce => CONTROL_ANNOUNCE_VERSION,
        }
    }

    pub(super) const fn action(self) -> &'static str {
        match self {
            Self::Refresh => "refresh its NNPages catalog",
            Self::Announce => "announce the hosted page destination",
        }
    }

    pub(super) const fn noun(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Announce => "announcement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NnPagesControlFailure {
    Scan,
    SourcePage,
    RouteUpdate,
    CatalogUnavailable,
    DestinationUnavailable,
    IndexUnavailable,
    AnnounceSend,
}

impl NnPagesControlFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::SourcePage => "source-page",
            Self::RouteUpdate => "route-update",
            Self::CatalogUnavailable => "catalog-unavailable",
            Self::DestinationUnavailable => "destination-unavailable",
            Self::IndexUnavailable => "index-unavailable",
            Self::AnnounceSend => "announce-send",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "scan" => Some(Self::Scan),
            "source-page" => Some(Self::SourcePage),
            "route-update" => Some(Self::RouteUpdate),
            "catalog-unavailable" => Some(Self::CatalogUnavailable),
            "destination-unavailable" => Some(Self::DestinationUnavailable),
            "index-unavailable" => Some(Self::IndexUnavailable),
            "announce-send" => Some(Self::AnnounceSend),
            _ => None,
        }
    }

    const fn valid_for(self, kind: NnPagesControlKind) -> bool {
        match kind {
            NnPagesControlKind::Refresh => matches!(
                self,
                Self::Scan
                    | Self::SourcePage
                    | Self::RouteUpdate
                    | Self::CatalogUnavailable
                    | Self::DestinationUnavailable
            ),
            NnPagesControlKind::Announce => matches!(
                self,
                Self::DestinationUnavailable | Self::IndexUnavailable | Self::AnnounceSend
            ),
        }
    }

    pub(super) const fn description(self) -> &'static str {
        match self {
            Self::Scan => "the hosted directories could not be scanned",
            Self::SourcePage => "the bundled source page could not be refreshed",
            Self::RouteUpdate => "a hosted route could not be updated",
            Self::CatalogUnavailable => "the NNPages catalog was unavailable",
            Self::DestinationUnavailable => "this daemon does not own the hosted page destination",
            Self::IndexUnavailable => "nnpages/pages/index.mu is not serveable",
            Self::AnnounceSend => "the announcement could not be sent",
        }
    }
}

impl NnPagesRefreshError {
    const fn control_failure(&self) -> NnPagesControlFailure {
        match self {
            Self::Scan(_) => NnPagesControlFailure::Scan,
            Self::SourcePage(_) => NnPagesControlFailure::SourcePage,
            Self::Runtime { .. } => NnPagesControlFailure::RouteUpdate,
            Self::CatalogPoisoned => NnPagesControlFailure::CatalogUnavailable,
            Self::DestinationUnavailable => NnPagesControlFailure::DestinationUnavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NnPagesControlOutcome {
    Refreshed(NnPagesRefreshReport),
    Announced,
    Failed(NnPagesControlFailure),
    Aborted,
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodedNnPagesControlRequest {
    id: u128,
    kind: NnPagesControlKind,
}

/// A control request whose durable marker has moved out of the pending namespace.
pub(crate) struct ClaimedNnPagesControlRequest {
    request: DecodedNnPagesControlRequest,
    inflight_path: PathBuf,
    result_path: PathBuf,
    terminal_committed: bool,
}

pub(crate) async fn next_control_request(
    config_dir: &Path,
) -> io::Result<ClaimedNnPagesControlRequest> {
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
            .filter_map(|entry| {
                let path = entry.path();
                decode_control_request(&path, CONTROL_REQUEST_PREFIX).map(|request| (path, request))
            })
            .collect::<Vec<_>>();
        requests.sort_by_key(|(_, request)| request.id);

        // Moving the file out of the pending namespace is the durable claim. Later polls only inspect request-* files, while inflight-* remains available for cancellation and restart recovery.
        for (path, request) in requests {
            let inflight_path = control_path(&control, CONTROL_INFLIGHT_PREFIX, request.id);
            match fs::rename(&path, &inflight_path) {
                Ok(()) => {
                    sync_parent_directory(&inflight_path)?;
                    return Ok(ClaimedNnPagesControlRequest {
                        request,
                        inflight_path,
                        result_path: control_path(&control, CONTROL_RESULT_PREFIX, request.id),
                        terminal_committed: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

impl ClaimedNnPagesControlRequest {
    pub(crate) fn kind(&self) -> NnPagesControlKind {
        self.request.kind
    }

    pub(crate) fn finish(
        mut self,
        result: Result<NnPagesRefreshReport, NnPagesRefreshError>,
    ) -> io::Result<()> {
        let outcome = match result {
            Ok(report) => NnPagesControlOutcome::Refreshed(report),
            Err(error) => NnPagesControlOutcome::Failed(error.control_failure()),
        };
        self.commit(outcome)
    }

    pub(crate) fn finish_announce(
        mut self,
        result: Result<(), NnPagesControlFailure>,
    ) -> io::Result<()> {
        let outcome = match result {
            Ok(()) => NnPagesControlOutcome::Announced,
            Err(failure) => NnPagesControlOutcome::Failed(failure),
        };
        self.commit(outcome)
    }

    fn commit(&mut self, outcome: NnPagesControlOutcome) -> io::Result<()> {
        let encoded = encode_control_outcome(self.request.kind, self.request.id, outcome)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid NNPages control outcome",
                )
            })?;
        if let Err(error) = atomic_control_write(&self.result_path, encoded.as_bytes()) {
            self.terminal_committed = control_file_matches(&self.result_path, encoded.as_bytes());
            return Err(error);
        }
        self.terminal_committed = true;
        remove_control_file(&self.inflight_path)
    }
}

impl Drop for ClaimedNnPagesControlRequest {
    fn drop(&mut self) {
        if self.terminal_committed {
            return;
        }
        let Some(encoded) = encode_control_outcome(
            self.request.kind,
            self.request.id,
            NnPagesControlOutcome::Aborted,
        ) else {
            return;
        };
        if let Err(error) = atomic_control_write(&self.result_path, encoded.as_bytes()) {
            self.terminal_committed = control_file_matches(&self.result_path, encoded.as_bytes());
            tracing::warn!(
                event = "nnpages_control_abort_result_failed",
                kind = ?self.request.kind,
                error = %error,
            );
            return;
        }
        self.terminal_committed = true;
        if let Err(error) = remove_control_file(&self.inflight_path) {
            tracing::warn!(
                event = "nnpages_control_abort_cleanup_failed",
                kind = ?self.request.kind,
                error = %error,
            );
        }
    }
}

pub(super) async fn request_refresh(
    config_dir: &Path,
) -> Result<NnPagesRefreshReport, NnPagesCliError> {
    match request_control(config_dir, NnPagesControlKind::Refresh).await? {
        NnPagesControlOutcome::Refreshed(report) => Ok(report),
        NnPagesControlOutcome::Failed(failure) => Err(NnPagesCliError::OperationFailed {
            kind: NnPagesControlKind::Refresh,
            failure,
        }),
        NnPagesControlOutcome::Aborted => Err(NnPagesCliError::OperationAborted(
            NnPagesControlKind::Refresh,
        )),
        NnPagesControlOutcome::Indeterminate => Err(NnPagesCliError::OperationIndeterminate(
            NnPagesControlKind::Refresh,
        )),
        NnPagesControlOutcome::Announced => Err(NnPagesCliError::InvalidResult),
    }
}

pub(super) async fn request_announce(config_dir: &Path) -> Result<(), NnPagesCliError> {
    match request_control(config_dir, NnPagesControlKind::Announce).await? {
        NnPagesControlOutcome::Announced => Ok(()),
        NnPagesControlOutcome::Failed(failure) => Err(NnPagesCliError::OperationFailed {
            kind: NnPagesControlKind::Announce,
            failure,
        }),
        NnPagesControlOutcome::Aborted => Err(NnPagesCliError::OperationAborted(
            NnPagesControlKind::Announce,
        )),
        NnPagesControlOutcome::Indeterminate => Err(NnPagesCliError::OperationIndeterminate(
            NnPagesControlKind::Announce,
        )),
        NnPagesControlOutcome::Refreshed(_) => Err(NnPagesCliError::InvalidResult),
    }
}

async fn request_control(
    config_dir: &Path,
    kind: NnPagesControlKind,
) -> Result<NnPagesControlOutcome, NnPagesCliError> {
    let control = control_root(config_dir);
    fs::create_dir_all(&control).map_err(NnPagesCliError::Control)?;
    let id = next_control_id();
    let request_path = control_path(&control, CONTROL_REQUEST_PREFIX, id);
    let inflight_path = control_path(&control, CONTROL_INFLIGHT_PREFIX, id);
    let result_path = control_path(&control, CONTROL_RESULT_PREFIX, id);
    let request = format!("{}\n{id:032x}\n", kind.version());
    atomic_control_write(&request_path, request.as_bytes()).map_err(NnPagesCliError::Control)?;

    let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
    loop {
        match fs::read_to_string(&result_path) {
            Ok(text) => {
                let outcome = decode_control_outcome(kind, id, &text);
                let _ = remove_control_file(&result_path);
                let _ = remove_control_file(&request_path);
                if outcome.is_some() {
                    let _ = remove_control_file(&inflight_path);
                }
                return outcome.ok_or(NnPagesCliError::InvalidResult);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = remove_control_file(&request_path);
                return Err(NnPagesCliError::Control(error));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = remove_control_file(&request_path);
            return Err(NnPagesCliError::TimedOut);
        }
        tokio::time::sleep(CONTROL_POLL_INTERVAL).await;
    }
}

fn control_root(config_dir: &Path) -> PathBuf {
    config_dir.join(CONTROL_DIRECTORY_NAME)
}

fn control_path(control: &Path, prefix: &str, id: u128) -> PathBuf {
    control.join(format!("{prefix}{id:032x}"))
}

fn remove_control_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn control_file_matches(path: &Path, expected: &[u8]) -> bool {
    fs::read(path).is_ok_and(|actual| actual == expected)
}

/// Converts work left by a previous daemon into terminal results without replaying external effects.
pub(crate) fn recover_control_state(config_dir: &Path) -> io::Result<()> {
    let control = control_root(config_dir);
    let entries = match fs::read_dir(&control) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut requests = Vec::new();
    let mut inflight = Vec::new();
    let mut results = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if control_file_id(&path, CONTROL_REQUEST_PREFIX).is_some() {
            requests.push(path);
        } else if control_file_id(&path, CONTROL_INFLIGHT_PREFIX).is_some() {
            inflight.push(path);
        } else if control_file_id(&path, CONTROL_RESULT_PREFIX).is_some() {
            results.push(path);
        }
    }

    for path in inflight {
        let Some(request) = decode_control_request(&path, CONTROL_INFLIGHT_PREFIX) else {
            discard_control_artifact(&path, "unsupported_or_malformed_inflight")?;
            continue;
        };
        recover_inflight_request(&control, &path, request)?;
    }
    for path in requests {
        if decode_control_request(&path, CONTROL_REQUEST_PREFIX).is_none() {
            discard_control_artifact(&path, "unsupported_or_malformed_request")?;
        }
    }
    for path in results {
        if decode_control_result_file(&path).is_none() {
            discard_control_artifact(&path, "unsupported_or_malformed_result")?;
        }
    }
    Ok(())
}

fn recover_inflight_request(
    control: &Path,
    inflight_path: &Path,
    request: DecodedNnPagesControlRequest,
) -> io::Result<()> {
    let result_path = control_path(control, CONTROL_RESULT_PREFIX, request.id);
    match fs::read_to_string(&result_path) {
        Ok(text) if decode_control_outcome(request.kind, request.id, &text).is_some() => {
            remove_control_file(inflight_path)?;
            tracing::info!(
                event = "nnpages_control_recovered",
                kind = ?request.kind,
                outcome = "terminal_result_preserved",
            );
            return Ok(());
        }
        Ok(_) => {
            tracing::warn!(
                event = "nnpages_control_result_replaced",
                kind = ?request.kind,
                cause = "invalid_result",
            );
            remove_control_file(&result_path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let encoded = encode_control_outcome(
        request.kind,
        request.id,
        NnPagesControlOutcome::Indeterminate,
    )
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid NNPages recovery outcome",
        )
    })?;
    atomic_control_write(&result_path, encoded.as_bytes())?;
    remove_control_file(inflight_path)?;
    tracing::warn!(
        event = "nnpages_control_recovered",
        kind = ?request.kind,
        outcome = "indeterminate",
    );
    Ok(())
}

fn discard_control_artifact(path: &Path, cause: &'static str) -> io::Result<()> {
    tracing::warn!(
        event = "nnpages_control_artifact_discarded",
        path = %path.display(),
        cause,
    );
    remove_control_file(path)
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

pub(super) fn atomic_control_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn control_file_id(path: &Path, prefix: &str) -> Option<u128> {
    let encoded = path.file_name()?.to_str()?.strip_prefix(prefix)?;
    if encoded.len() != 32 {
        return None;
    }
    u128::from_str_radix(encoded, 16).ok()
}

fn control_kind_from_version(version: &str) -> Option<NnPagesControlKind> {
    match version {
        CONTROL_REFRESH_VERSION => Some(NnPagesControlKind::Refresh),
        CONTROL_ANNOUNCE_VERSION => Some(NnPagesControlKind::Announce),
        _ => None,
    }
}

fn decode_control_request(path: &Path, prefix: &str) -> Option<DecodedNnPagesControlRequest> {
    let id = control_file_id(path, prefix)?;
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let kind = control_kind_from_version(lines.next()?)?;
    if u128::from_str_radix(lines.next()?, 16).ok()? != id || lines.next().is_some() {
        return None;
    }
    Some(DecodedNnPagesControlRequest { id, kind })
}

fn decode_control_result_file(path: &Path) -> Option<NnPagesControlOutcome> {
    let id = control_file_id(path, CONTROL_RESULT_PREFIX)?;
    let text = fs::read_to_string(path).ok()?;
    let kind = control_kind_from_version(text.lines().next()?)?;
    decode_control_outcome(kind, id, &text)
}

fn encode_control_outcome(
    kind: NnPagesControlKind,
    id: u128,
    outcome: NnPagesControlOutcome,
) -> Option<String> {
    match outcome {
        NnPagesControlOutcome::Refreshed(report) if kind == NnPagesControlKind::Refresh => {
            Some(format!(
                "{}\n{id:032x}\nok\n{}\n{}\n{}\n{}\n{}\n{}\n",
                kind.version(),
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
            ))
        }
        NnPagesControlOutcome::Announced if kind == NnPagesControlKind::Announce => {
            Some(format!("{}\n{id:032x}\nok\n", kind.version()))
        }
        NnPagesControlOutcome::Failed(failure) if failure.valid_for(kind) => Some(format!(
            "{}\n{id:032x}\nfailed\n{}\n",
            kind.version(),
            failure.code(),
        )),
        NnPagesControlOutcome::Aborted => Some(format!("{}\n{id:032x}\naborted\n", kind.version())),
        NnPagesControlOutcome::Indeterminate => {
            Some(format!("{}\n{id:032x}\nindeterminate\n", kind.version()))
        }
        NnPagesControlOutcome::Refreshed(_)
        | NnPagesControlOutcome::Announced
        | NnPagesControlOutcome::Failed(_) => None,
    }
}

fn decode_control_outcome(
    kind: NnPagesControlKind,
    id: u128,
    text: &str,
) -> Option<NnPagesControlOutcome> {
    let mut lines = text.lines();
    if lines.next()? != kind.version() || u128::from_str_radix(lines.next()?, 16).ok()? != id {
        return None;
    }
    match lines.next() {
        Some("ok") if kind == NnPagesControlKind::Refresh => {
            let outcome = NnPagesControlOutcome::Refreshed(NnPagesRefreshReport {
                discovered: parse_control_count(lines.next())?,
                added: parse_control_count(lines.next())?,
                removed: parse_control_count(lines.next())?,
                unchanged: parse_control_count(lines.next())?,
                settings_status: NnPagesSettingsStatus::from_control_value(lines.next()?)?,
                settings_changed: match lines.next()? {
                    "changed" => true,
                    "unchanged" => false,
                    _ => return None,
                },
            });
            if lines.next().is_some() {
                return None;
            }
            Some(outcome)
        }
        Some("ok") if kind == NnPagesControlKind::Announce && lines.next().is_none() => {
            Some(NnPagesControlOutcome::Announced)
        }
        Some("failed") => {
            let failure = NnPagesControlFailure::from_code(lines.next()?)?;
            if !failure.valid_for(kind) || lines.next().is_some() {
                return None;
            }
            Some(NnPagesControlOutcome::Failed(failure))
        }
        Some("aborted") if lines.next().is_none() => Some(NnPagesControlOutcome::Aborted),
        Some("indeterminate") if lines.next().is_none() => {
            Some(NnPagesControlOutcome::Indeterminate)
        }
        _ => None,
    }
}

fn parse_control_count(value: Option<&str>) -> Option<usize> {
    value?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_requests_decode_with_their_own_version() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let id = 0x2au128;
        let path = control_path(directory.path(), CONTROL_REQUEST_PREFIX, id);
        fs::write(&path, format!("{CONTROL_ANNOUNCE_VERSION}\n{id:032x}\n")).expect("request");
        let request = decode_control_request(&path, CONTROL_REQUEST_PREFIX).expect("decodes");
        assert_eq!(request.kind, NnPagesControlKind::Announce);
    }

    #[tokio::test]
    async fn config_local_refresh_control_returns_the_daemon_report() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_refresh(&client_dir).await });
        let pending =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
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

    /// One operator command must produce exactly one dispatch.
    ///
    /// The daemon re-enters this poll as soon as it has spawned the work, so a request left in the pending namespace can be dispatched again while the first task is still running.
    /// The duplicate refresh absorbed the change the operator's own refresh was waiting to report, and the same control path could put a duplicate announcement on the air.
    #[tokio::test]
    async fn a_control_request_is_dispatched_only_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_refresh(&client_dir).await });
        let pending =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");

        // The dispatched work has not finished, but the request is no longer pending.
        assert!(
            tokio::time::timeout(CONTROL_POLL_INTERVAL * 5, next_control_request(&config_dir))
                .await
                .is_err(),
            "an in-flight request was dispatched a second time"
        );

        let report = NnPagesRefreshReport {
            discovered: 2,
            added: 0,
            removed: 1,
            unchanged: 2,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: false,
        };
        pending.finish(Ok(report)).expect("result written");
        assert_eq!(
            client.await.expect("client joins").expect("refresh"),
            report
        );
    }

    #[test]
    fn v2_control_outcomes_round_trip_success_and_every_failure_code() {
        let report = NnPagesRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
            settings_status: NnPagesSettingsStatus::InvalidDefaults,
            settings_changed: false,
        };
        let refreshed = NnPagesControlOutcome::Refreshed(report);
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                7,
                &encode_control_outcome(NnPagesControlKind::Refresh, 7, refreshed)
                    .expect("refresh encodes"),
            ),
            Some(refreshed)
        );
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Announce,
                8,
                &encode_control_outcome(
                    NnPagesControlKind::Announce,
                    8,
                    NnPagesControlOutcome::Announced,
                )
                .expect("announce encodes"),
            ),
            Some(NnPagesControlOutcome::Announced)
        );

        for failure in [
            NnPagesControlFailure::Scan,
            NnPagesControlFailure::SourcePage,
            NnPagesControlFailure::RouteUpdate,
            NnPagesControlFailure::CatalogUnavailable,
            NnPagesControlFailure::DestinationUnavailable,
        ] {
            let outcome = NnPagesControlOutcome::Failed(failure);
            let encoded = encode_control_outcome(NnPagesControlKind::Refresh, 9, outcome)
                .expect("refresh failure encodes");
            assert_eq!(
                decode_control_outcome(NnPagesControlKind::Refresh, 9, &encoded),
                Some(outcome)
            );
        }
        for failure in [
            NnPagesControlFailure::DestinationUnavailable,
            NnPagesControlFailure::IndexUnavailable,
            NnPagesControlFailure::AnnounceSend,
        ] {
            let outcome = NnPagesControlOutcome::Failed(failure);
            let encoded = encode_control_outcome(NnPagesControlKind::Announce, 10, outcome)
                .expect("announce failure encodes");
            assert_eq!(
                decode_control_outcome(NnPagesControlKind::Announce, 10, &encoded),
                Some(outcome)
            );
        }
        for outcome in [
            NnPagesControlOutcome::Aborted,
            NnPagesControlOutcome::Indeterminate,
        ] {
            let encoded = encode_control_outcome(NnPagesControlKind::Refresh, 11, outcome)
                .expect("terminal state encodes");
            assert_eq!(
                decode_control_outcome(NnPagesControlKind::Refresh, 11, &encoded),
                Some(outcome)
            );
        }
    }

    #[test]
    fn v2_control_outcomes_reject_wrong_identity_version_status_and_code() {
        let report = NnPagesControlOutcome::Refreshed(NnPagesRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: false,
        });
        let encoded =
            encode_control_outcome(NnPagesControlKind::Refresh, 7, report).expect("report encodes");
        assert_eq!(
            decode_control_outcome(NnPagesControlKind::Refresh, 8, &encoded),
            None
        );
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                7,
                "prnsd-nnpages-refresh-v1\n00000000000000000000000000000007\naborted\n",
            ),
            None
        );
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                7,
                "prnsd-nnpages-refresh-v2\n00000000000000000000000000000007\nunknown\n",
            ),
            None
        );
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                7,
                "prnsd-nnpages-refresh-v2\n00000000000000000000000000000007\nfailed\nunknown\n",
            ),
            None
        );
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                7,
                "prnsd-nnpages-refresh-v2\n00000000000000000000000000000007\nfailed\nindex-unavailable\n",
            ),
            None
        );
    }

    fn write_test_control_request(
        config_dir: &Path,
        kind: NnPagesControlKind,
        id: u128,
    ) -> (PathBuf, PathBuf, PathBuf) {
        let control = control_root(config_dir);
        fs::create_dir_all(&control).expect("control directory");
        let request_path = control_path(&control, CONTROL_REQUEST_PREFIX, id);
        let inflight_path = control_path(&control, CONTROL_INFLIGHT_PREFIX, id);
        let result_path = control_path(&control, CONTROL_RESULT_PREFIX, id);
        atomic_control_write(
            &request_path,
            format!("{}\n{id:032x}\n", kind.version()).as_bytes(),
        )
        .expect("control request");
        (request_path, inflight_path, result_path)
    }

    #[tokio::test]
    async fn known_failures_reach_the_cli_with_specific_codes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_refresh(&client_dir).await });
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("refresh arrives")
                .expect("refresh is valid");
        request
            .finish(Err(NnPagesRefreshError::DestinationUnavailable))
            .expect("refresh failure result");
        assert!(matches!(
            client.await.expect("refresh client joins"),
            Err(NnPagesCliError::OperationFailed {
                kind: NnPagesControlKind::Refresh,
                failure: NnPagesControlFailure::DestinationUnavailable,
            })
        ));

        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_announce(&client_dir).await });
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("announce arrives")
                .expect("announce is valid");
        request
            .finish_announce(Err(NnPagesControlFailure::IndexUnavailable))
            .expect("announce failure result");
        assert!(matches!(
            client.await.expect("announce client joins"),
            Err(NnPagesCliError::OperationFailed {
                kind: NnPagesControlKind::Announce,
                failure: NnPagesControlFailure::IndexUnavailable,
            })
        ));
    }

    #[tokio::test]
    async fn dropping_claimed_work_reports_aborted_and_removes_its_marker() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let client_dir = config_dir.clone();
        let client = tokio::spawn(async move { request_refresh(&client_dir).await });
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");
        let inflight_path = request.inflight_path.clone();
        assert!(inflight_path.is_file());

        drop(request);

        assert!(matches!(
            client.await.expect("client joins"),
            Err(NnPagesCliError::OperationAborted(
                NnPagesControlKind::Refresh
            ))
        ));
        assert!(!inflight_path.exists());
    }

    #[tokio::test]
    async fn an_unwritable_result_leaves_the_inflight_marker_for_recovery() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let id = 0x31u128;
        let (_, inflight_path, result_path) =
            write_test_control_request(&config_dir, NnPagesControlKind::Refresh, id);
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");
        fs::create_dir(&result_path).expect("blocking result directory");
        let report = NnPagesRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: false,
        };

        request.finish(Ok(report)).expect_err("result write fails");

        assert!(inflight_path.is_file());
        assert!(result_path.is_dir());
    }

    #[tokio::test]
    async fn an_unremovable_marker_does_not_overwrite_a_committed_result() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let id = 0x32u128;
        let (_, inflight_path, result_path) =
            write_test_control_request(&config_dir, NnPagesControlKind::Refresh, id);
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");
        fs::remove_file(&inflight_path).expect("replace marker");
        fs::create_dir(&inflight_path).expect("blocking marker directory");
        let report = NnPagesRefreshReport {
            discovered: 1,
            added: 1,
            removed: 0,
            unchanged: 0,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: false,
        };

        request
            .finish(Ok(report))
            .expect_err("marker cleanup fails");

        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                id,
                &fs::read_to_string(result_path).expect("committed result"),
            ),
            Some(NnPagesControlOutcome::Refreshed(report))
        );
    }

    #[tokio::test]
    async fn recovery_preserves_a_committed_result_and_only_clears_its_marker() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let id = 0x41u128;
        let (_, inflight_path, result_path) =
            write_test_control_request(&config_dir, NnPagesControlKind::Refresh, id);
        let request =
            tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                .await
                .expect("request arrives")
                .expect("request is valid");
        let outcome = NnPagesControlOutcome::Refreshed(NnPagesRefreshReport {
            discovered: 2,
            added: 1,
            removed: 0,
            unchanged: 1,
            settings_status: NnPagesSettingsStatus::Loaded,
            settings_changed: false,
        });
        let encoded = encode_control_outcome(NnPagesControlKind::Refresh, id, outcome)
            .expect("result encodes");
        atomic_control_write(&result_path, encoded.as_bytes()).expect("result commits");
        std::mem::forget(request);

        recover_control_state(&config_dir).expect("control recovery");

        assert!(!inflight_path.exists());
        assert_eq!(
            decode_control_outcome(
                NnPagesControlKind::Refresh,
                id,
                &fs::read_to_string(result_path).expect("result remains"),
            ),
            Some(outcome)
        );
    }

    #[tokio::test]
    async fn recovery_marks_refresh_and_announce_indeterminate_without_replaying_them() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path().to_path_buf();
        let controls = [
            (0x51u128, NnPagesControlKind::Refresh),
            (0x52u128, NnPagesControlKind::Announce),
        ];
        for (id, kind) in controls {
            write_test_control_request(&config_dir, kind, id);
            let request =
                tokio::time::timeout(Duration::from_secs(2), next_control_request(&config_dir))
                    .await
                    .expect("request arrives")
                    .expect("request is valid");
            assert_eq!(request.kind(), kind);
            std::mem::forget(request);
        }

        recover_control_state(&config_dir).expect("control recovery");

        let control = control_root(&config_dir);
        for (id, kind) in controls {
            let result_path = control_path(&control, CONTROL_RESULT_PREFIX, id);
            assert_eq!(
                decode_control_outcome(
                    kind,
                    id,
                    &fs::read_to_string(result_path).expect("recovery result"),
                ),
                Some(NnPagesControlOutcome::Indeterminate)
            );
        }
        assert!(
            tokio::time::timeout(CONTROL_POLL_INTERVAL * 5, next_control_request(&config_dir),)
                .await
                .is_err(),
            "recovery replayed an in-flight request"
        );
    }

    #[test]
    fn recovery_discards_unsupported_and_malformed_exact_control_artifacts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_dir = directory.path();
        let control = control_root(config_dir);
        fs::create_dir_all(&control).expect("control directory");
        let legacy_request = control_path(&control, CONTROL_REQUEST_PREFIX, 0x61);
        let legacy_inflight = control_path(&control, CONTROL_INFLIGHT_PREFIX, 0x62);
        let legacy_result = control_path(&control, CONTROL_RESULT_PREFIX, 0x63);
        let malformed_request = control_path(&control, CONTROL_REQUEST_PREFIX, 0x64);
        let valid_request = control_path(&control, CONTROL_REQUEST_PREFIX, 0x65);
        fs::write(
            &legacy_request,
            "prnsd-nnpages-refresh-v1\n00000000000000000000000000000061\n",
        )
        .expect("legacy request");
        fs::write(
            &legacy_inflight,
            "prnsd-nnpages-announce-v1\n00000000000000000000000000000062\n",
        )
        .expect("legacy inflight");
        fs::write(
            &legacy_result,
            "prnsd-nnpages-refresh-v1\n00000000000000000000000000000063\nfailed\n",
        )
        .expect("legacy result");
        fs::write(
            &malformed_request,
            "prnsd-nnpages-refresh-v2\n000000000000000000000000000000ff\n",
        )
        .expect("malformed request");
        fs::write(
            &valid_request,
            "prnsd-nnpages-refresh-v2\n00000000000000000000000000000065\n",
        )
        .expect("valid request");

        recover_control_state(config_dir).expect("control recovery");

        for path in [
            legacy_request,
            legacy_inflight,
            legacy_result,
            malformed_request,
        ] {
            assert!(!path.exists(), "{} was not discarded", path.display());
        }
        assert!(valid_request.is_file());
    }

    #[test]
    fn cli_errors_distinguish_known_aborted_and_indeterminate_outcomes() {
        assert!(NnPagesCliError::OperationFailed {
            kind: NnPagesControlKind::Announce,
            failure: NnPagesControlFailure::IndexUnavailable,
        }
        .to_string()
        .contains("index.mu is not serveable"));
        assert!(
            NnPagesCliError::OperationAborted(NnPagesControlKind::Refresh)
                .to_string()
                .contains("aborted")
        );
        assert!(
            NnPagesCliError::OperationIndeterminate(NnPagesControlKind::Refresh)
                .to_string()
                .contains("safe to retry")
        );
        assert!(
            NnPagesCliError::OperationIndeterminate(NnPagesControlKind::Announce)
                .to_string()
                .contains("may already have aired")
        );
    }
}
