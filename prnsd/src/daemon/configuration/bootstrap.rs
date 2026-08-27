use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use personal_rns::config::editing::{ConfigEdit, ConfigEditError, ConfigFile, ConfigFileError};
use personal_rns::config::{discover, DiscoveryError};

const CONFIG_FILE_NAME: &str = "config";
const DEFAULT_BACKBONE_PORT: u16 = 4242;
const DEFAULT_WEBSOCKET_PORT: u16 = 4284;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const LISTEN_PORT: &str = "PRNSD_BACKBONE_LISTEN_PORT";
const BACKBONE_DISCOVERABLE: &str = "PRNSD_BACKBONE_DISCOVERABLE";
const REACHABLE_HOST: &str = "PRNSD_REACHABLE_HOST";
const REACHABLE_PORT: &str = "PRNSD_REACHABLE_PORT";
const RAILWAY_HOST: &str = "RAILWAY_TCP_PROXY_DOMAIN";
const RAILWAY_PORT: &str = "RAILWAY_TCP_PROXY_PORT";
const NNPAGES_ANNOUNCE: &str = "PRNSD_NNPAGES_ANNOUNCE";
const NNPAGES_ANNOUNCE_INTERVAL_MINUTES: &str = "PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES";
const SOURCE_ARCHIVE_ENV: &str = "PRNSD_SOURCE_ARCHIVE";
const SOURCE_ARCHIVE_RECEIPT_FILE_NAME: &str = ".source.zip.prnsd-managed";
const SOURCE_ARCHIVE_RECEIPT_VERSION: &str = "prnsd-managed-source-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedEndpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerBootstrapEnvironment {
    listen_port: u16,
    backbone_discoverable: bool,
    published: Option<PublishedEndpoint>,
    nnpages_announce: bool,
    nnpages_announce_interval_minutes: u64,
}

impl ServerBootstrapEnvironment {
    fn from_process() -> Result<Self, ServerBootstrapError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, ServerBootstrapError> {
        let listen_port = match lookup(LISTEN_PORT) {
            Some(value) => parse_port(LISTEN_PORT, value)?,
            None => DEFAULT_BACKBONE_PORT,
        };
        let explicit = endpoint_pair(
            REACHABLE_HOST,
            lookup(REACHABLE_HOST),
            REACHABLE_PORT,
            lookup(REACHABLE_PORT),
        )?;
        let railway = endpoint_pair(
            RAILWAY_HOST,
            lookup(RAILWAY_HOST),
            RAILWAY_PORT,
            lookup(RAILWAY_PORT),
        )?;
        let published = explicit.or(railway);
        let backbone_discoverable = match lookup(BACKBONE_DISCOVERABLE) {
            Some(value) => parse_bool(BACKBONE_DISCOVERABLE, value)?,
            None => published.is_some(),
        };
        if backbone_discoverable && published.is_none() {
            return Err(ServerBootstrapError::PublishedEndpointRequired {
                control: BACKBONE_DISCOVERABLE,
            });
        }
        let nnpages_announce = match lookup(NNPAGES_ANNOUNCE) {
            Some(value) => parse_bool(NNPAGES_ANNOUNCE, value)?,
            None => true,
        };
        let nnpages_announce_interval_minutes = match lookup(NNPAGES_ANNOUNCE_INTERVAL_MINUTES) {
            Some(value) => parse_positive_minutes(NNPAGES_ANNOUNCE_INTERVAL_MINUTES, value)?,
            None => crate::nnpages::DEFAULT_ANNOUNCE_INTERVAL_MINUTES,
        };
        Ok(Self {
            listen_port,
            backbone_discoverable,
            published,
            nnpages_announce,
            nnpages_announce_interval_minutes,
        })
    }

    fn render(&self) -> String {
        let mut config = format!(
            "[reticulum]\n\
             enable_transport = Yes\n\
             share_instance = Yes\n\
             \n\
             [interfaces]\n\
             [[Cloud Backbone]]\n\
             type = BackboneInterface\n\
             interface_enabled = Yes\n\
             listen_ip = 0.0.0.0\n\
             listen_port = {}\n",
            self.listen_port,
        );
        match (self.backbone_discoverable, &self.published) {
            (true, Some(endpoint)) => {
                config.push_str(&format!(
                    "discoverable = Yes\n\
                     reachable_on = {}\n\
                     reachable_port = {}\n",
                    endpoint.host, endpoint.port
                ));
            }
            (false, _) => config.push_str("discoverable = No\n"),
            (true, None) => unreachable!("validated bootstrap discovery endpoint"),
        }
        config.push_str(&format!(
            "\n[[WebSocket Server]]\n\
             type = PrnsWebSocketServer\n\
             interface_enabled = Yes\n\
             listen_ip = 0.0.0.0\n\
             listen_port = {DEFAULT_WEBSOCKET_PORT}\n"
        ));
        config
    }

    fn render_nnpages_settings(&self) -> String {
        format!(
            "announce = {}\nannounce_interval_minutes = {}\n",
            self.nnpages_announce, self.nnpages_announce_interval_minutes,
        )
    }
}

pub(super) struct ServerBootstrapReceipt {
    pub(super) config_path: PathBuf,
    pub(super) seeded_page: Option<PathBuf>,
}

pub(super) fn create_server_config_if_missing(
    override_dir: Option<&Path>,
) -> Result<Option<ServerBootstrapReceipt>, ServerBootstrapError> {
    let discovered = discover(override_dir).map_err(ServerBootstrapError::Discover)?;
    if discovered.config.is_some() {
        return Ok(None);
    }
    let environment = ServerBootstrapEnvironment::from_process()?;
    let path = discovered.dir.join(CONFIG_FILE_NAME);
    let mut transaction = NnPagesSeedTransaction::default();
    match materialize_nnpages_settings(&discovered.dir, &environment.render_nnpages_settings()) {
        Ok(Some(path)) => transaction.record(NnPagesSeedChange::Created(path)),
        Ok(None) => {}
        Err(error) => return Err(transaction.rollback(error)),
    }
    if let Err(error) = prepare_nnpages_layout(&discovered.dir) {
        return Err(transaction.rollback(error));
    }
    match stage_bootstrap_source(&discovered.dir) {
        Ok(Some(seed)) => {
            for path in seed.created {
                transaction.record(NnPagesSeedChange::Created(path));
            }
        }
        Ok(None) => {}
        Err(error) => return Err(transaction.rollback(error)),
    }
    let page = match seed_default_page(&discovered.dir) {
        Ok(page) => page,
        Err(error) => return Err(transaction.rollback(error)),
    };
    if let Some(path) = &page {
        transaction.record(NnPagesSeedChange::Created(path.clone()));
    }
    match seed_source_page_tracked(&discovered.dir) {
        Ok(result) => transaction.record_optional(result.change),
        Err(error) => return Err(transaction.rollback(error)),
    }
    match seed_coming_from_rns_page_tracked(&discovered.dir) {
        Ok(result) => transaction.record_optional(result.change),
        Err(error) => return Err(transaction.rollback(error)),
    }
    if let Err(error) = materialize(&path, &environment.render()) {
        return Err(transaction.rollback(error));
    }
    Ok(Some(ServerBootstrapReceipt {
        config_path: path,
        seeded_page: page,
    }))
}

fn stage_bootstrap_source(
    config_dir: &Path,
) -> Result<Option<SourceArchiveSeed>, ServerBootstrapError> {
    let target =
        crate::nnpages::file_root(config_dir).join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME);
    match fs::symlink_metadata(&target) {
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(nnpages_storage(
                "inspect hosted source archive",
                &target,
                error,
            ))
        }
    }
    stage_bundled_source(config_dir, false)
}

fn materialize(path: &Path, fallback: &str) -> Result<(), ServerBootstrapError> {
    let file = ConfigFile::load(path, fallback).map_err(ServerBootstrapError::ConfigFile)?;
    if file.is_materialized() {
        return Ok(());
    }
    let candidate = file
        .document()
        .edit(&ConfigEdit::Batch(Vec::new()))
        .map_err(ServerBootstrapError::ConfigEdit)?;
    file.write(&candidate)
        .map_err(ServerBootstrapError::ConfigFile)?;
    Ok(())
}

pub(crate) fn materialize_nnpages_settings(
    config_dir: &Path,
    contents: &str,
) -> Result<Option<PathBuf>, ServerBootstrapError> {
    let root = crate::nnpages::root(config_dir);
    prepare_nnpages_directory(&root)?;
    let path = crate::nnpages::settings_path(config_dir);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => return Ok(None),
        Ok(_) => {
            return Err(ServerBootstrapError::InvalidNnPagesTarget {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(nnpages_storage("inspect NNPages settings", &path, source));
        }
    }

    let (mut file, staging_path) = create_staging_file(&root)?;
    let result = (|| {
        file.write_all(contents.as_bytes())
            .map_err(|source| nnpages_storage("write NNPages settings", &staging_path, source))?;
        file.sync_all()
            .map_err(|source| nnpages_storage("sync NNPages settings", &staging_path, source))?;
        drop(file);
        match fs::hard_link(&staging_path, &path) {
            Ok(()) => {
                let mut transaction = NnPagesSeedTransaction::default();
                transaction.record(NnPagesSeedChange::Created(path.clone()));
                match sync_nnpages_directory(&root) {
                    Ok(()) => Ok(Some(path.clone())),
                    Err(error) => Err(transaction.rollback(error)),
                }
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                validate_page_target(&path)?;
                Ok(None)
            }
            Err(source) => Err(nnpages_storage("publish NNPages settings", &path, source)),
        }
    })();
    let _ = fs::remove_file(staging_path);
    result
}

const SOURCE_PAGE_MARKER: &str = "# prnsd:managed:source-page";
const SOURCE_PAGE_MISSING: &str = include_str!("../../../assets/nnpages/source_missing.mu");
const SOURCE_PAGE_AVAILABLE: &str = include_str!("../../../../assets/nnpages/source_available.mu");
const COMING_FROM_RNS_MARKER: &str = "# prnsd:managed:coming-from-rns";
const COMING_FROM_RNS_PAGE: &str = include_str!("../../../../assets/nnpages/coming_from_rns.mu");
const SOURCE_CHECKSUM_LINE: &str =
    "`F999Verify:`f `F6eb`_`[source.zip.sha256`:/file/source.zip.sha256]`_`f\n\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourcePageState {
    ArchiveMissing,
    ArchiveStaged {
        archive_bytes: u64,
        has_checksum: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourcePageSeed {
    Written {
        path: PathBuf,
        state: SourcePageState,
    },
    Unchanged(SourcePageState),
    OperatorOwned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedPageSeed {
    Written(PathBuf),
    Unchanged,
    OperatorOwned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceArchiveSeed {
    pub(crate) archive_path: PathBuf,
    pub(crate) archive_bytes: u64,
    pub(crate) created: Vec<PathBuf>,
    pub(crate) replaced: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BundledSourceRefresh {
    NotStaged,
    OperatorOwned,
    BundleUnavailable,
    Current(SourceArchiveSeed),
    Updated(SourceArchiveSeed),
}

pub(crate) fn stage_bundled_source(
    config_dir: &Path,
    required: bool,
) -> Result<Option<SourceArchiveSeed>, ServerBootstrapError> {
    let Some(source) = locate_bundled_source(required)? else {
        return Ok(None);
    };
    stage_source_archive(config_dir, &source).map(Some)
}

pub(crate) fn stage_source_archive(
    config_dir: &Path,
    source: &Path,
) -> Result<SourceArchiveSeed, ServerBootstrapError> {
    let source_file = open_source_archive(source)?;
    let metadata = source_file
        .metadata()
        .map_err(|error| nnpages_storage("inspect source archive", source, error))?;
    if !metadata.file_type().is_file() {
        return Err(ServerBootstrapError::InvalidSourceArchive {
            path: source.to_path_buf(),
        });
    }
    if metadata.len() > crate::nnpages::MAX_FILE_BYTES {
        return Err(ServerBootstrapError::SourceArchiveTooLarge {
            path: source.to_path_buf(),
            bytes: metadata.len(),
            maximum: crate::nnpages::MAX_FILE_BYTES,
        });
    }
    let mut archive = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or_default());
    Read::take(source_file, crate::nnpages::MAX_FILE_BYTES + 1)
        .read_to_end(&mut archive)
        .map_err(|error| nnpages_storage("read source archive", source, error))?;
    if u64::try_from(archive.len()).unwrap_or(u64::MAX) > crate::nnpages::MAX_FILE_BYTES {
        return Err(ServerBootstrapError::SourceArchiveTooLarge {
            path: source.to_path_buf(),
            bytes: u64::try_from(archive.len()).unwrap_or(u64::MAX),
            maximum: crate::nnpages::MAX_FILE_BYTES,
        });
    }
    let digest = data_encoding::HEXLOWER.encode(&personal_rns::crypto::sha256(&archive));
    validate_source_checksum_if_present(source, &digest)?;
    let checksum = source_checksum_document(&archive);

    let root = crate::nnpages::file_root(config_dir);
    prepare_nnpages_directory(&root)?;
    let archive_path = root.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME);
    let checksum_path = root.join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME);
    let receipt_path = root.join(SOURCE_ARCHIVE_RECEIPT_FILE_NAME);
    let receipt = source_archive_receipt(&archive);
    let plans = source_archive_publish_plans(
        &archive_path,
        &checksum_path,
        &receipt_path,
        &archive,
        &checksum,
        &receipt,
    )?;
    let mut created = Vec::new();
    let mut replaced = Vec::new();
    let mut transaction = NnPagesSeedTransaction::default();
    let result = (|| {
        apply_source_publish_plan(
            &root,
            &archive_path,
            &archive,
            plans.archive,
            &mut created,
            &mut replaced,
            &mut transaction,
        )?;
        apply_source_publish_plan(
            &root,
            &checksum_path,
            &checksum,
            plans.checksum,
            &mut created,
            &mut replaced,
            &mut transaction,
        )?;
        apply_source_publish_plan(
            &root,
            &receipt_path,
            &receipt,
            plans.receipt,
            &mut created,
            &mut replaced,
            &mut transaction,
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        return Err(transaction.rollback(error));
    }
    Ok(SourceArchiveSeed {
        archive_path,
        archive_bytes: u64::try_from(archive.len()).unwrap_or(u64::MAX),
        created,
        replaced,
    })
}

pub(crate) fn refresh_staged_bundled_source(
    config_dir: &Path,
) -> Result<BundledSourceRefresh, ServerBootstrapError> {
    let root = crate::nnpages::file_root(config_dir);
    let archive_path = root.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME);
    let checksum_path = root.join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME);
    let receipt_path = root.join(SOURCE_ARCHIVE_RECEIPT_FILE_NAME);
    let Some(archive) = read_hosted_file(&archive_path)? else {
        return Ok(BundledSourceRefresh::NotStaged);
    };
    let Some(checksum) = read_hosted_file(&checksum_path)? else {
        return Ok(BundledSourceRefresh::OperatorOwned);
    };
    let Some(receipt) = read_hosted_file(&receipt_path)? else {
        return Ok(BundledSourceRefresh::OperatorOwned);
    };
    if checksum != source_checksum_document(&archive) || receipt != source_archive_receipt(&archive)
    {
        return Ok(BundledSourceRefresh::OperatorOwned);
    }
    let Some(source) = locate_bundled_source(false)? else {
        return Ok(BundledSourceRefresh::BundleUnavailable);
    };
    let staged = stage_source_archive(config_dir, &source)?;
    if staged.replaced.is_empty() {
        Ok(BundledSourceRefresh::Current(staged))
    } else {
        Ok(BundledSourceRefresh::Updated(staged))
    }
}

#[derive(Debug)]
struct SourceArchivePublishPlans {
    archive: SourcePublishPlan,
    checksum: SourcePublishPlan,
    receipt: SourcePublishPlan,
}

#[derive(Debug)]
enum SourcePublishPlan {
    Current,
    Create,
    Replace(Vec<u8>),
}

fn source_archive_publish_plans(
    archive_path: &Path,
    checksum_path: &Path,
    receipt_path: &Path,
    desired_archive: &[u8],
    desired_checksum: &[u8],
    desired_receipt: &[u8],
) -> Result<SourceArchivePublishPlans, ServerBootstrapError> {
    let existing_archive = read_hosted_file(archive_path)?;
    let existing_checksum = read_hosted_file(checksum_path)?;
    let existing_receipt = read_hosted_file(receipt_path)?;
    match (existing_archive, existing_checksum, existing_receipt) {
        (None, None, None) => Ok(SourceArchivePublishPlans {
            archive: SourcePublishPlan::Create,
            checksum: SourcePublishPlan::Create,
            receipt: SourcePublishPlan::Create,
        }),
        (Some(archive), checksum, receipt) if archive == desired_archive => {
            let checksum = match checksum {
                None => SourcePublishPlan::Create,
                Some(checksum) if checksum == desired_checksum => SourcePublishPlan::Current,
                Some(_) => {
                    return Err(ServerBootstrapError::HostedFileConflict {
                        path: checksum_path.to_path_buf(),
                    })
                }
            };
            let receipt = match receipt {
                None => SourcePublishPlan::Create,
                Some(receipt) if receipt == desired_receipt => SourcePublishPlan::Current,
                Some(_) => {
                    return Err(ServerBootstrapError::HostedFileConflict {
                        path: receipt_path.to_path_buf(),
                    })
                }
            };
            Ok(SourceArchivePublishPlans {
                archive: SourcePublishPlan::Current,
                checksum,
                receipt,
            })
        }
        (Some(archive), Some(checksum), receipt)
            if checksum == source_checksum_document(&archive) =>
        {
            let receipt = match receipt {
                None => SourcePublishPlan::Create,
                Some(receipt) if receipt == source_archive_receipt(&archive) => {
                    SourcePublishPlan::Replace(receipt)
                }
                Some(_) => {
                    return Err(ServerBootstrapError::HostedFileConflict {
                        path: receipt_path.to_path_buf(),
                    })
                }
            };
            Ok(SourceArchivePublishPlans {
                archive: SourcePublishPlan::Replace(archive),
                checksum: SourcePublishPlan::Replace(checksum),
                receipt,
            })
        }
        (Some(_), _, _) => Err(ServerBootstrapError::HostedFileConflict {
            path: archive_path.to_path_buf(),
        }),
        (None, Some(_), _) => Err(ServerBootstrapError::HostedFileConflict {
            path: checksum_path.to_path_buf(),
        }),
        (None, None, Some(_)) => Err(ServerBootstrapError::HostedFileConflict {
            path: receipt_path.to_path_buf(),
        }),
    }
}

fn apply_source_publish_plan(
    root: &Path,
    path: &Path,
    desired: &[u8],
    plan: SourcePublishPlan,
    created: &mut Vec<PathBuf>,
    replaced: &mut Vec<PathBuf>,
    transaction: &mut NnPagesSeedTransaction,
) -> Result<(), ServerBootstrapError> {
    match plan {
        SourcePublishPlan::Current => Ok(()),
        SourcePublishPlan::Create => {
            if publish_file_once(root, path, desired)? {
                created.push(path.to_path_buf());
                transaction.record(NnPagesSeedChange::Created(path.to_path_buf()));
            }
            Ok(())
        }
        SourcePublishPlan::Replace(previous) => {
            replace_file(root, path, desired)?;
            replaced.push(path.to_path_buf());
            transaction.record(NnPagesSeedChange::Replaced {
                path: path.to_path_buf(),
                previous,
            });
            Ok(())
        }
    }
}

fn read_hosted_file(path: &Path) -> Result<Option<Vec<u8>>, ServerBootstrapError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && metadata.len() <= crate::nnpages::MAX_FILE_BYTES =>
        {
            fs::read(path)
                .map(Some)
                .map_err(|error| nnpages_storage("read hosted file", path, error))
        }
        Ok(_) => Err(ServerBootstrapError::HostedFileConflict {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(nnpages_storage("inspect hosted file", path, error)),
    }
}

fn source_checksum_document(archive: &[u8]) -> Vec<u8> {
    let digest = data_encoding::HEXLOWER.encode(&personal_rns::crypto::sha256(archive));
    format!("{digest}  {}\n", crate::nnpages::SOURCE_ARCHIVE_FILE_NAME).into_bytes()
}

fn source_archive_receipt(archive: &[u8]) -> Vec<u8> {
    let digest = data_encoding::HEXLOWER.encode(&personal_rns::crypto::sha256(archive));
    format!("{SOURCE_ARCHIVE_RECEIPT_VERSION}\n{digest}\n").into_bytes()
}

#[cfg(unix)]
fn open_source_archive(path: &Path) -> Result<File, ServerBootstrapError> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)
    .map_err(|error| nnpages_storage("open source archive", path, error))
}

#[cfg(not(unix))]
fn open_source_archive(path: &Path) -> Result<File, ServerBootstrapError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| nnpages_storage("inspect source archive", path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ServerBootstrapError::InvalidSourceArchive {
            path: path.to_path_buf(),
        });
    }
    File::open(path).map_err(|error| nnpages_storage("open source archive", path, error))
}

fn locate_bundled_source(required: bool) -> Result<Option<PathBuf>, ServerBootstrapError> {
    if let Some(configured) = std::env::var_os(SOURCE_ARCHIVE_ENV) {
        let configured = PathBuf::from(configured);
        if configured.as_os_str().is_empty() {
            return Err(ServerBootstrapError::SourceArchiveUnavailable {
                searched: vec![configured],
            });
        }
        return Ok(Some(configured));
    }
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME));
        }
    }
    #[cfg(unix)]
    candidates.push(PathBuf::from("/usr/share/prnsd/source.zip"));
    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| candidate.is_file() && !candidate.is_symlink())
    {
        return Ok(Some(candidate.clone()));
    }
    if required {
        return Err(ServerBootstrapError::SourceArchiveUnavailable {
            searched: candidates,
        });
    }
    Ok(None)
}

fn validate_source_checksum_if_present(
    archive: &Path,
    digest: &str,
) -> Result<(), ServerBootstrapError> {
    let checksum_path = archive.with_file_name(format!(
        "{}.sha256",
        archive.file_name().unwrap_or_default().to_string_lossy()
    ));
    match fs::symlink_metadata(&checksum_path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let document = fs::read_to_string(&checksum_path)
                .map_err(|error| nnpages_storage("read source checksum", &checksum_path, error))?;
            if document.split_whitespace().next() != Some(digest) {
                return Err(ServerBootstrapError::SourceChecksumMismatch {
                    path: checksum_path,
                });
            }
            Ok(())
        }
        Ok(_) => Err(ServerBootstrapError::InvalidSourceArchive {
            path: checksum_path,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(nnpages_storage(
            "inspect source checksum",
            &checksum_path,
            error,
        )),
    }
}

fn publish_file_once(root: &Path, path: &Path, bytes: &[u8]) -> Result<bool, ServerBootstrapError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let existing =
                fs::read(path).map_err(|error| nnpages_storage("read hosted file", path, error))?;
            if existing == bytes {
                return Ok(false);
            }
            return Err(ServerBootstrapError::HostedFileConflict {
                path: path.to_path_buf(),
            });
        }
        Ok(_) => {
            return Err(ServerBootstrapError::HostedFileConflict {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(nnpages_storage("inspect hosted file", path, error)),
    }

    let (mut file, staging_path) = create_staging_file(root)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|error| nnpages_storage("write hosted file", &staging_path, error))?;
        file.sync_all()
            .map_err(|error| nnpages_storage("sync hosted file", &staging_path, error))?;
        drop(file);
        match fs::hard_link(&staging_path, path) {
            Ok(()) => {
                let mut transaction = NnPagesSeedTransaction::default();
                transaction.record(NnPagesSeedChange::Created(path.to_path_buf()));
                match sync_nnpages_directory(root) {
                    Ok(()) => Ok(true),
                    Err(error) => Err(transaction.rollback(error)),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = fs::read(path)
                    .map_err(|error| nnpages_storage("read hosted file", path, error))?;
                if existing == bytes {
                    Ok(false)
                } else {
                    Err(ServerBootstrapError::HostedFileConflict {
                        path: path.to_path_buf(),
                    })
                }
            }
            Err(error) => Err(nnpages_storage("publish hosted file", path, error)),
        }
    })();
    let _ = fs::remove_file(staging_path);
    result
}

pub(crate) fn seed_source_page(config_dir: &Path) -> Result<SourcePageSeed, ServerBootstrapError> {
    Ok(seed_source_page_tracked(config_dir)?.outcome)
}

struct TrackedSeed<T> {
    outcome: T,
    change: Option<NnPagesSeedChange>,
}

fn seed_source_page_tracked(
    config_dir: &Path,
) -> Result<TrackedSeed<SourcePageSeed>, ServerBootstrapError> {
    let state = source_archive_state(config_dir);
    let seeded = seed_marked_page(
        config_dir,
        crate::nnpages::SOURCE_PAGE_FILE_NAME,
        SOURCE_PAGE_MARKER,
        &render_source_page(state),
    )?;
    let outcome = match seeded.outcome {
        ManagedPageSeed::Written(path) => SourcePageSeed::Written { path, state },
        ManagedPageSeed::Unchanged => SourcePageSeed::Unchanged(state),
        ManagedPageSeed::OperatorOwned => SourcePageSeed::OperatorOwned,
    };
    Ok(TrackedSeed {
        outcome,
        change: seeded.change,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourcePageRefresh {
    Rewritten(SourcePageState),
    Unchanged,
    OperatorOwned,
    Absent,
}

pub(crate) fn refresh_source_page(
    config_dir: &Path,
) -> Result<SourcePageRefresh, ServerBootstrapError> {
    let path = crate::nnpages::page_root(config_dir).join(crate::nnpages::SOURCE_PAGE_FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Ok(SourcePageRefresh::OperatorOwned),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SourcePageRefresh::Absent)
        }
        Err(source) => return Err(nnpages_storage("inspect managed page", &path, source)),
    }
    Ok(match seed_source_page(config_dir)? {
        SourcePageSeed::Written { state, .. } => SourcePageRefresh::Rewritten(state),
        SourcePageSeed::Unchanged(_) => SourcePageRefresh::Unchanged,
        SourcePageSeed::OperatorOwned => SourcePageRefresh::OperatorOwned,
    })
}

pub(crate) fn seed_coming_from_rns_page(
    config_dir: &Path,
) -> Result<ManagedPageSeed, ServerBootstrapError> {
    Ok(seed_coming_from_rns_page_tracked(config_dir)?.outcome)
}

fn seed_coming_from_rns_page_tracked(
    config_dir: &Path,
) -> Result<TrackedSeed<ManagedPageSeed>, ServerBootstrapError> {
    seed_marked_page(
        config_dir,
        crate::nnpages::COMING_FROM_RNS_PAGE_FILE_NAME,
        COMING_FROM_RNS_MARKER,
        COMING_FROM_RNS_PAGE,
    )
}

fn seed_marked_page(
    config_dir: &Path,
    file_name: &str,
    marker: &str,
    desired: &str,
) -> Result<TrackedSeed<ManagedPageSeed>, ServerBootstrapError> {
    let root = crate::nnpages::page_root(config_dir);
    prepare_nnpages_directory(&root)?;
    let path = root.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let current = fs::read(&path)
                .map_err(|source| nnpages_storage("read managed page", &path, source))?;
            if !carries_marker(&current, marker) {
                return Ok(TrackedSeed {
                    outcome: ManagedPageSeed::OperatorOwned,
                    change: None,
                });
            }
            if current == desired.as_bytes() {
                return Ok(TrackedSeed {
                    outcome: ManagedPageSeed::Unchanged,
                    change: None,
                });
            }
            replace_file(&root, &path, desired.as_bytes())?;
            return Ok(TrackedSeed {
                outcome: ManagedPageSeed::Written(path.clone()),
                change: Some(NnPagesSeedChange::Replaced {
                    path,
                    previous: current,
                }),
            });
        }
        Ok(_) => {
            return Ok(TrackedSeed {
                outcome: ManagedPageSeed::OperatorOwned,
                change: None,
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(nnpages_storage("inspect managed page", &path, source)),
    }
    replace_file(&root, &path, desired.as_bytes())?;
    Ok(TrackedSeed {
        outcome: ManagedPageSeed::Written(path.clone()),
        change: Some(NnPagesSeedChange::Created(path)),
    })
}

#[derive(Default)]
struct NnPagesSeedTransaction {
    changes: Vec<NnPagesSeedChange>,
}

enum NnPagesSeedChange {
    Created(PathBuf),
    Replaced { path: PathBuf, previous: Vec<u8> },
}

impl NnPagesSeedTransaction {
    fn record(&mut self, change: NnPagesSeedChange) {
        self.changes.push(change);
    }

    fn record_optional(&mut self, change: Option<NnPagesSeedChange>) {
        if let Some(change) = change {
            self.record(change);
        }
    }

    fn rollback(mut self, original: ServerBootstrapError) -> ServerBootstrapError {
        let mut first_failure = None;
        while let Some(change) = self.changes.pop() {
            let rollback_path = match &change {
                NnPagesSeedChange::Created(path) | NnPagesSeedChange::Replaced { path, .. } => {
                    path.clone()
                }
            };
            let result = match change {
                NnPagesSeedChange::Created(path) => match fs::remove_file(&path) {
                    Ok(()) => sync_parent_directory(&path),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                },
                NnPagesSeedChange::Replaced { path, previous } => path
                    .parent()
                    .ok_or_else(|| io::Error::other("managed page has no parent directory"))
                    .and_then(|root| {
                        replace_file(root, &path, &previous).map_err(|error| match error {
                            ServerBootstrapError::NnPagesStorage { source, .. } => source,
                            other => io::Error::other(other.to_string()),
                        })
                    }),
            };
            if let Err(source) = result {
                first_failure.get_or_insert((rollback_path, source));
            }
        }
        if let Some((path, source)) = first_failure {
            return ServerBootstrapError::RollbackFailed {
                original: Box::new(original),
                path,
                source,
            };
        }
        original
    }
}

fn sync_parent_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("managed page has no parent directory"))?;
        File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn carries_marker(bytes: &[u8], marker: &str) -> bool {
    bytes
        .strip_prefix(marker.as_bytes())
        .is_some_and(|rest| rest.starts_with(b"\n") || rest.starts_with(b"\r\n"))
}

fn source_archive_state(config_dir: &Path) -> SourcePageState {
    let Some(archive_bytes) =
        crate::nnpages::served_file_len(config_dir, crate::nnpages::SOURCE_ARCHIVE_FILE_NAME)
    else {
        return SourcePageState::ArchiveMissing;
    };
    SourcePageState::ArchiveStaged {
        archive_bytes,
        has_checksum: crate::nnpages::served_file_len(
            config_dir,
            crate::nnpages::SOURCE_CHECKSUM_FILE_NAME,
        )
        .is_some(),
    }
}

fn render_source_page(state: SourcePageState) -> String {
    match state {
        SourcePageState::ArchiveMissing => SOURCE_PAGE_MISSING.to_owned(),
        SourcePageState::ArchiveStaged {
            archive_bytes,
            has_checksum,
        } => {
            let checksum_line = match has_checksum {
                true => SOURCE_CHECKSUM_LINE,
                false => "\n",
            };
            SOURCE_PAGE_AVAILABLE
                .replace("{{SIZE}}", &format_archive_size(archive_bytes))
                .replace("{{CHECKSUM_LINE}}\n", checksum_line)
                .replace(
                    "{{SOURCE_COMMIT_LINE}}\n",
                    &format!(
                        "`F999Daemon build:`f `F6ebPrns v{} · commit {}`f\n\n",
                        crate::build_identity::VERSION,
                        crate::build_identity::COMMIT,
                    ),
                )
        }
    }
}

pub(crate) fn format_archive_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let (scaled_tenths, unit) = if bytes < 1024 * 1024 {
        (bytes * 10 / 1024, "KB")
    } else if bytes < 1024 * 1024 * 1024 {
        (bytes * 10 / (1024 * 1024), "MB")
    } else {
        (bytes * 10 / (1024 * 1024 * 1024), "GB")
    };
    format!("{}.{} {unit}", scaled_tenths / 10, scaled_tenths % 10)
}

fn replace_file(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), ServerBootstrapError> {
    let (mut file, staging_path) = create_staging_file(root)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|source| nnpages_storage("write staging file", &staging_path, source))?;
        file.sync_all()
            .map_err(|source| nnpages_storage("sync staging file", &staging_path, source))?;
        drop(file);
        crate::nnpages::replace_file(&staging_path, path)
            .map_err(|source| nnpages_storage("publish file", path, source))?;
        sync_nnpages_directory(root)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging_path);
    }
    result
}

pub(crate) fn seed_default_page(
    config_dir: &Path,
) -> Result<Option<PathBuf>, ServerBootstrapError> {
    let root = crate::nnpages::page_root(config_dir);
    prepare_nnpages_directory(&root)?;
    let path = root.join(crate::nnpages::INDEX_FILE_NAME);
    if path.exists() {
        validate_page_target(&path)?;
        return Ok(None);
    }

    let staging = create_staging_file(&root)?;
    let staging_path = staging.1;
    let mut file = staging.0;
    let result = (|| {
        file.write_all(crate::nnpages::DEFAULT_INDEX_PAGE)
            .map_err(|source| nnpages_storage("write staging page", &staging_path, source))?;
        file.sync_all()
            .map_err(|source| nnpages_storage("sync staging page", &staging_path, source))?;
        drop(file);
        match fs::hard_link(&staging_path, &path) {
            Ok(()) => {
                let mut transaction = NnPagesSeedTransaction::default();
                transaction.record(NnPagesSeedChange::Created(path.clone()));
                if let Err(error) = sync_nnpages_directory(&root) {
                    return Err(transaction.rollback(error));
                }
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                validate_page_target(&path)?;
                return Ok(None);
            }
            Err(source) => {
                return Err(nnpages_storage("publish page", &path, source));
            }
        }
        Ok(Some(path.clone()))
    })();
    let _ = fs::remove_file(&staging_path);
    result
}

fn prepare_nnpages_directory(root: &Path) -> Result<(), ServerBootstrapError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            return Err(ServerBootstrapError::InvalidNnPagesTarget {
                path: root.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(nnpages_storage("inspect page directory", root, source)),
    }
    fs::create_dir_all(root)
        .map_err(|source| nnpages_storage("create page directory", root, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|source| nnpages_storage("protect page directory", root, source))?;
    }
    Ok(())
}

pub(crate) fn prepare_nnpages_layout(config_dir: &Path) -> Result<(), ServerBootstrapError> {
    prepare_nnpages_directory(&crate::nnpages::root(config_dir))?;
    prepare_nnpages_directory(&crate::nnpages::page_root(config_dir))?;
    prepare_nnpages_directory(&crate::nnpages::file_root(config_dir))
}

fn validate_page_target(path: &Path) -> Result<(), ServerBootstrapError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| nnpages_storage("inspect existing page", path, source))?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(ServerBootstrapError::InvalidNnPagesTarget {
            path: path.to_path_buf(),
        })
    }
}

fn create_staging_file(root: &Path) -> Result<(File, PathBuf), ServerBootstrapError> {
    for _ in 0..64 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".{}.tmp-{}-{sequence}",
            crate::nnpages::INDEX_FILE_NAME,
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(nnpages_storage("create staging page", &path, source)),
        }
    }
    Err(nnpages_storage(
        "create staging page",
        root,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique staging filename",
        ),
    ))
}

#[cfg(unix)]
fn sync_nnpages_directory(root: &Path) -> Result<(), ServerBootstrapError> {
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| nnpages_storage("sync page directory", root, source))
}

#[cfg(not(unix))]
fn sync_nnpages_directory(_root: &Path) -> Result<(), ServerBootstrapError> {
    Ok(())
}

fn nnpages_storage(
    operation: &'static str,
    path: &Path,
    source: io::Error,
) -> ServerBootstrapError {
    ServerBootstrapError::NnPagesStorage {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn endpoint_pair(
    host_name: &'static str,
    host: Option<OsString>,
    port_name: &'static str,
    port: Option<OsString>,
) -> Result<Option<PublishedEndpoint>, ServerBootstrapError> {
    match (host, port) {
        (None, None) => Ok(None),
        (Some(host), Some(port)) => Ok(Some(PublishedEndpoint {
            host: parse_host(host_name, host)?,
            port: parse_port(port_name, port)?,
        })),
        (Some(_), None) => Err(ServerBootstrapError::IncompleteEndpoint {
            present: host_name,
            missing: port_name,
        }),
        (None, Some(_)) => Err(ServerBootstrapError::IncompleteEndpoint {
            present: port_name,
            missing: host_name,
        }),
    }
}

fn parse_host(name: &'static str, value: OsString) -> Result<String, ServerBootstrapError> {
    let value = value
        .into_string()
        .map_err(|_| ServerBootstrapError::NonUtf8 { name })?;
    let host = value.trim();
    if host.is_empty()
        || host != value
        || !host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'[' | b']')
        })
    {
        return Err(ServerBootstrapError::InvalidHost { name, value });
    }
    Ok(host.to_string())
}

fn parse_port(name: &'static str, value: OsString) -> Result<u16, ServerBootstrapError> {
    let value = value
        .into_string()
        .map_err(|_| ServerBootstrapError::NonUtf8 { name })?;
    let port = value
        .parse::<u16>()
        .map_err(|_| ServerBootstrapError::InvalidPort {
            name,
            value: value.clone(),
        })?;
    if port == 0 {
        return Err(ServerBootstrapError::InvalidPort { name, value });
    }
    Ok(port)
}

fn parse_bool(name: &'static str, value: OsString) -> Result<bool, ServerBootstrapError> {
    let value = value
        .into_string()
        .map_err(|_| ServerBootstrapError::NonUtf8 { name })?;
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(ServerBootstrapError::InvalidBoolean { name, value }),
    }
}

fn parse_positive_minutes(
    name: &'static str,
    value: OsString,
) -> Result<u64, ServerBootstrapError> {
    let value = value
        .into_string()
        .map_err(|_| ServerBootstrapError::NonUtf8 { name })?;
    let minutes = value
        .parse::<u64>()
        .ok()
        .filter(|minutes| *minutes != 0 && minutes.checked_mul(60).is_some())
        .ok_or_else(|| ServerBootstrapError::InvalidMinutes {
            name,
            value: value.clone(),
        })?;
    Ok(minutes)
}

#[derive(Debug)]
pub(crate) enum ServerBootstrapError {
    Discover(DiscoveryError),
    NonUtf8 {
        name: &'static str,
    },
    IncompleteEndpoint {
        present: &'static str,
        missing: &'static str,
    },
    InvalidHost {
        name: &'static str,
        value: String,
    },
    InvalidPort {
        name: &'static str,
        value: String,
    },
    InvalidBoolean {
        name: &'static str,
        value: String,
    },
    InvalidMinutes {
        name: &'static str,
        value: String,
    },
    PublishedEndpointRequired {
        control: &'static str,
    },
    ConfigFile(ConfigFileError),
    ConfigEdit(ConfigEditError),
    NnPagesStorage {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    InvalidNnPagesTarget {
        path: PathBuf,
    },
    InvalidSourceArchive {
        path: PathBuf,
    },
    SourceArchiveTooLarge {
        path: PathBuf,
        bytes: u64,
        maximum: u64,
    },
    SourceArchiveUnavailable {
        searched: Vec<PathBuf>,
    },
    SourceChecksumMismatch {
        path: PathBuf,
    },
    HostedFileConflict {
        path: PathBuf,
    },
    RollbackFailed {
        original: Box<ServerBootstrapError>,
        path: PathBuf,
        source: io::Error,
    },
}

impl core::fmt::Display for ServerBootstrapError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Discover(error) => error.fmt(formatter),
            Self::NonUtf8 { name } => write!(formatter, "{name} is not valid UTF-8"),
            Self::IncompleteEndpoint { present, missing } => write!(
                formatter,
                "{present} was supplied without required companion {missing}"
            ),
            Self::InvalidHost { name, value } => {
                write!(formatter, "{name} contains an invalid host value {value:?}")
            }
            Self::InvalidPort { name, value } => {
                write!(
                    formatter,
                    "{name} must be a port from 1 through 65535, got {value:?}"
                )
            }
            Self::InvalidBoolean { name, value } => write!(
                formatter,
                "{name} must be Yes or No (true, false, on, off, 1, and 0 are also accepted), got {value:?}"
            ),
            Self::InvalidMinutes { name, value } => write!(
                formatter,
                "{name} must be a positive whole number of minutes representable by the host, got {value:?}"
            ),
            Self::PublishedEndpointRequired { control } => write!(
                formatter,
                "{control}=Yes requires a complete PRNSD_REACHABLE_HOST/PRNSD_REACHABLE_PORT or RAILWAY_TCP_PROXY_DOMAIN/RAILWAY_TCP_PROXY_PORT pair"
            ),
            Self::ConfigFile(error) => error.fmt(formatter),
            Self::ConfigEdit(error) => error.fmt(formatter),
            Self::NnPagesStorage {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {} failed: {source}", path.display()),
            Self::InvalidNnPagesTarget { path } => write!(
                formatter,
                "NNPages target {} is not a regular file or directory",
                path.display()
            ),
            Self::InvalidSourceArchive { path } => {
                write!(formatter, "{} is not a regular source archive", path.display())
            }
            Self::SourceArchiveTooLarge {
                path,
                bytes,
                maximum,
            } => write!(
                formatter,
                "source archive {} is {bytes} bytes; hosted files are limited to {maximum} bytes",
                path.display()
            ),
            Self::SourceArchiveUnavailable { searched } => {
                formatter.write_str(
                    "no bundled source archive is available; set PRNSD_SOURCE_ARCHIVE or pass --source-archive",
                )?;
                if !searched.is_empty() {
                    formatter.write_str(" (searched")?;
                    for path in searched {
                        write!(formatter, " {}", path.display())?;
                    }
                    formatter.write_str(")")?;
                }
                Ok(())
            }
            Self::SourceChecksumMismatch { path } => write!(
                formatter,
                "source checksum {} does not match the archive",
                path.display()
            ),
            Self::HostedFileConflict { path } => write!(
                formatter,
                "hosted file {} already exists with different operator-owned bytes",
                path.display()
            ),
            Self::RollbackFailed {
                original,
                path,
                source,
            } => write!(
                formatter,
                "{original}; rollback of {} also failed: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ServerBootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Discover(error) => Some(error),
            Self::ConfigFile(error) => Some(error),
            Self::ConfigEdit(error) => Some(error),
            Self::NnPagesStorage { source, .. } => Some(source),
            Self::RollbackFailed { source, .. } => Some(source),
            Self::NonUtf8 { .. }
            | Self::IncompleteEndpoint { .. }
            | Self::InvalidHost { .. }
            | Self::InvalidPort { .. }
            | Self::InvalidBoolean { .. }
            | Self::InvalidMinutes { .. }
            | Self::PublishedEndpointRequired { .. }
            | Self::InvalidNnPagesTarget { .. }
            | Self::InvalidSourceArchive { .. }
            | Self::SourceArchiveTooLarge { .. }
            | Self::SourceArchiveUnavailable { .. }
            | Self::SourceChecksumMismatch { .. }
            | Self::HostedFileConflict { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use personal_rns::config::{
        parse_and_plan, DiscoveryAdvertisementPlan, PlannedMedium, TcpListenHost,
    };
    use personal_rns::interfaces::websocket::WebSocketFramingSelection;

    use super::*;

    fn environment(
        values: &[(&str, &str)],
    ) -> Result<ServerBootstrapEnvironment, ServerBootstrapError> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect::<BTreeMap<_, _>>();
        ServerBootstrapEnvironment::from_lookup(|name| values.get(name).cloned())
    }

    #[test]
    fn generic_endpoint_precedes_railway_and_listener_defaults() {
        let environment = environment(&[
            (REACHABLE_HOST, "backbone.example"),
            (REACHABLE_PORT, "443"),
            (RAILWAY_HOST, "railway.example"),
            (RAILWAY_PORT, "10001"),
        ])
        .expect("environment is valid");

        assert_eq!(environment.listen_port, DEFAULT_BACKBONE_PORT);
        assert!(environment.backbone_discoverable);
        assert!(environment.nnpages_announce);
        assert_eq!(
            environment.nnpages_announce_interval_minutes,
            crate::nnpages::DEFAULT_ANNOUNCE_INTERVAL_MINUTES
        );
        assert_eq!(
            environment.published,
            Some(PublishedEndpoint {
                host: "backbone.example".to_string(),
                port: 443,
            })
        );
    }

    #[test]
    fn every_supplied_endpoint_pair_is_validated_before_precedence() {
        assert!(matches!(
            environment(&[
                (REACHABLE_HOST, "backbone.example"),
                (REACHABLE_PORT, "443"),
                (RAILWAY_HOST, "railway.example"),
            ]),
            Err(ServerBootstrapError::IncompleteEndpoint {
                present: RAILWAY_HOST,
                missing: RAILWAY_PORT,
            })
        ));
        assert!(matches!(
            environment(&[
                (REACHABLE_HOST, "backbone.example"),
                (REACHABLE_PORT, "443"),
                (RAILWAY_HOST, "railway.example"),
                (RAILWAY_PORT, "0"),
            ]),
            Err(ServerBootstrapError::InvalidPort {
                name: RAILWAY_PORT,
                ..
            })
        ));
    }

    #[test]
    fn partial_endpoints_and_zero_ports_fail_closed() {
        assert!(matches!(
            environment(&[(REACHABLE_HOST, "backbone.example")]),
            Err(ServerBootstrapError::IncompleteEndpoint { .. })
        ));
        assert!(matches!(
            environment(&[(LISTEN_PORT, "0")]),
            Err(ServerBootstrapError::InvalidPort { .. })
        ));
        assert!(matches!(
            environment(&[(BACKBONE_DISCOVERABLE, "Yes")]),
            Err(ServerBootstrapError::PublishedEndpointRequired {
                control: BACKBONE_DISCOVERABLE,
            })
        ));
        assert!(matches!(
            environment(&[(BACKBONE_DISCOVERABLE, "sometimes")]),
            Err(ServerBootstrapError::InvalidBoolean {
                name: BACKBONE_DISCOVERABLE,
                ..
            })
        ));
        assert!(matches!(
            environment(&[(NNPAGES_ANNOUNCE_INTERVAL_MINUTES, "0")]),
            Err(ServerBootstrapError::InvalidMinutes {
                name: NNPAGES_ANNOUNCE_INTERVAL_MINUTES,
                ..
            })
        ));
    }

    #[test]
    fn deployment_controls_separate_backbone_and_nnpages_settings() {
        let environment = environment(&[
            (BACKBONE_DISCOVERABLE, "No"),
            (REACHABLE_HOST, "backbone.example"),
            (REACHABLE_PORT, "443"),
            (NNPAGES_ANNOUNCE, "off"),
            (NNPAGES_ANNOUNCE_INTERVAL_MINUTES, "720"),
        ])
        .expect("environment is valid");
        assert!(!environment.backbone_discoverable);
        assert!(!environment.nnpages_announce);
        assert_eq!(environment.nnpages_announce_interval_minutes, 720);

        let plan = parse_and_plan(&environment.render())
            .expect("rendered configuration plans")
            .value;
        assert!(matches!(
            plan.interfaces[0].discovery,
            personal_rns::config::InterfaceDiscoveryPlan::Disabled
        ));
        assert!(!environment.render().contains("nnpages"));
        assert!(!environment.render().contains("announce_node_page"));

        let directory = tempfile::tempdir().expect("temporary directory");
        materialize_nnpages_settings(directory.path(), &environment.render_nnpages_settings())
            .expect("NNPages settings materialize");
        let catalog = crate::nnpages::NnPagesCatalog::empty(directory.path());
        let snapshot = catalog.settings_snapshot().expect("settings snapshot");
        assert_eq!(
            snapshot.status(),
            crate::nnpages::NnPagesSettingsStatus::Loaded
        );
        assert!(!snapshot.effective().announce());
        assert_eq!(snapshot.effective().announce_interval_minutes(), 720);
    }

    #[test]
    fn server_bootstrap_includes_the_default_websocket_listener() {
        let environment = environment(&[]).expect("default environment is valid");
        let plan = parse_and_plan(&environment.render())
            .expect("rendered configuration plans")
            .value;

        assert_eq!(plan.interfaces.len(), 2);
        assert!(matches!(
            &plan.interfaces[1].medium,
            PlannedMedium::PrnsWebSocketServer {
                listener,
                framing: WebSocketFramingSelection::Auto,
            }
                if listener.host == TcpListenHost::Address("0.0.0.0".to_string())
                    && listener.port == DEFAULT_WEBSOCKET_PORT
        ));
    }

    #[test]
    fn retired_page_environment_names_do_not_control_nnpages() {
        let environment = environment(&[
            ("PRNSD_NODE_PAGE_ANNOUNCE", "No"),
            ("PRNSD_NODE_PAGE_ANNOUNCE_INTERVAL", "720"),
        ])
        .expect("retired names are unrelated inputs");
        assert!(environment.nnpages_announce);
        assert_eq!(
            environment.nnpages_announce_interval_minutes,
            crate::nnpages::DEFAULT_ANNOUNCE_INTERVAL_MINUTES
        );
    }

    #[test]
    fn rendered_public_endpoint_uses_the_published_port() {
        let environment = environment(&[
            (LISTEN_PORT, "4242"),
            (RAILWAY_HOST, "mesh.up.railway.app"),
            (RAILWAY_PORT, "18443"),
        ])
        .expect("environment is valid");
        let plan = parse_and_plan(&environment.render())
            .expect("rendered configuration plans")
            .value;

        let personal_rns::config::InterfaceDiscoveryPlan::Announce(announcement) =
            &plan.interfaces[0].discovery
        else {
            panic!("cloud backbone must be discoverable");
        };
        assert_eq!(
            announcement.advertisement,
            DiscoveryAdvertisementPlan::Backbone {
                reachable_on: "mesh.up.railway.app".to_string(),
                port: 18443,
            }
        );
        assert_eq!(
            environment.render_nnpages_settings(),
            crate::nnpages::DEFAULT_SETTINGS_DOCUMENT
        );
    }

    #[test]
    fn materialization_is_owner_only_and_never_rewrites_existing_config() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(CONFIG_FILE_NAME);
        materialize(
            &path,
            &ServerBootstrapEnvironment {
                listen_port: 4242,
                backbone_discoverable: false,
                published: None,
                nnpages_announce: true,
                nnpages_announce_interval_minutes:
                    crate::nnpages::DEFAULT_ANNOUNCE_INTERVAL_MINUTES,
            }
            .render(),
        )
        .expect("configuration materializes");
        let first = std::fs::read(&path).expect("configuration is readable");

        materialize(&path, "this is deliberately not configuration")
            .expect("an existing configuration is untouched");
        assert_eq!(
            std::fs::read(&path).expect("configuration remains readable"),
            first
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("configuration metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn nnpages_settings_are_owner_only_and_never_rewrite_operator_content() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let created = materialize_nnpages_settings(
            directory.path(),
            "announce = false\nannounce_interval_minutes = 720\n",
        )
        .expect("settings materialize")
        .expect("settings are new");
        assert_eq!(created, crate::nnpages::settings_path(directory.path()));
        assert_eq!(
            std::fs::read_to_string(&created).expect("settings are readable"),
            "announce = false\nannounce_interval_minutes = 720\n"
        );

        std::fs::write(&created, "announce = true\n").expect("operator changes settings");
        assert!(
            materialize_nnpages_settings(directory.path(), "announce = false\n")
                .expect("existing settings are accepted")
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(&created).expect("operator settings remain"),
            "announce = true\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&created)
                    .expect("settings metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn default_nnpages_seed_establishes_the_complete_editable_layout() {
        let directory = tempfile::tempdir().expect("temporary directory");
        prepare_nnpages_layout(directory.path()).expect("NNPages layout");
        let settings = materialize_nnpages_settings(
            directory.path(),
            crate::nnpages::DEFAULT_SETTINGS_DOCUMENT,
        )
        .expect("default settings")
        .expect("settings are created");

        assert_eq!(
            std::fs::read_to_string(&settings).expect("settings are readable"),
            crate::nnpages::DEFAULT_SETTINGS_DOCUMENT
        );
        assert!(crate::nnpages::page_root(directory.path()).is_dir());
        assert!(crate::nnpages::file_root(directory.path()).is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(settings)
                    .expect("settings metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn settings_materialization_preserves_existing_bytes_even_when_invalid() {
        for existing in [
            b"announce = false\nannounce_interval_minutes = 720\n".as_slice(),
            b"this is not = valid = TOML\n".as_slice(),
            b"# operator-owned formatting\nannounce=false\n".as_slice(),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            prepare_nnpages_layout(directory.path()).expect("NNPages layout");
            let path = crate::nnpages::settings_path(directory.path());
            std::fs::write(&path, existing).expect("existing settings");

            assert!(materialize_nnpages_settings(
                directory.path(),
                crate::nnpages::DEFAULT_SETTINGS_DOCUMENT,
            )
            .expect("existing settings are accepted")
            .is_none());
            assert_eq!(
                std::fs::read(path).expect("settings remain readable"),
                existing
            );
        }
    }

    #[test]
    fn bootstrap_layout_has_dedicated_page_and_file_roots() {
        let directory = tempfile::tempdir().expect("temporary directory");
        prepare_nnpages_layout(directory.path()).expect("NNPages layout");

        assert!(crate::nnpages::root(directory.path()).is_dir());
        assert!(crate::nnpages::page_root(directory.path()).is_dir());
        assert!(crate::nnpages::file_root(directory.path()).is_dir());
        assert!(!directory.path().join("pages").exists());
        assert!(!directory.path().join("files").exists());
    }

    #[test]
    fn default_page_is_seeded_once_and_remains_operator_owned() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let page = seed_default_page(directory.path())
            .expect("page seeding succeeds")
            .expect("page is newly seeded");
        assert_eq!(
            std::fs::read(&page).expect("seeded page is readable"),
            crate::nnpages::DEFAULT_INDEX_PAGE
        );

        std::fs::write(&page, b"operator edition").expect("operator edits page");
        assert_eq!(
            seed_default_page(directory.path()).expect("existing page is accepted"),
            None
        );
        assert_eq!(
            std::fs::read(&page).expect("operator page is readable"),
            b"operator edition"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&page)
                    .expect("page metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn existing_config_prevents_deleted_page_from_being_reseeded() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = directory.path().join(CONFIG_FILE_NAME);
        materialize(
            &config,
            &ServerBootstrapEnvironment {
                listen_port: DEFAULT_BACKBONE_PORT,
                backbone_discoverable: false,
                published: None,
                nnpages_announce: true,
                nnpages_announce_interval_minutes:
                    crate::nnpages::DEFAULT_ANNOUNCE_INTERVAL_MINUTES,
            }
            .render(),
        )
        .expect("configuration");
        let page = seed_default_page(directory.path())
            .expect("page")
            .expect("new page");
        std::fs::remove_file(&page).expect("operator disables page");

        assert!(create_server_config_if_missing(Some(directory.path()))
            .expect("existing configuration")
            .is_none());
        assert!(!page.exists());
    }

    #[test]
    fn unsafe_existing_page_target_fails_bootstrap() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = crate::nnpages::page_root(directory.path());
        std::fs::create_dir_all(&root).expect("page root");
        std::fs::create_dir(root.join(crate::nnpages::INDEX_FILE_NAME))
            .expect("invalid page directory");

        assert!(matches!(
            seed_default_page(directory.path()),
            Err(ServerBootstrapError::InvalidNnPagesTarget { .. })
        ));
    }

    #[test]
    fn the_source_page_notes_a_missing_archive_and_flips_when_one_is_staged() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let SourcePageSeed::Written { path, state } =
            seed_source_page(directory.path()).expect("source page seeding succeeds")
        else {
            panic!("a fresh directory writes the source page");
        };
        assert_eq!(state, SourcePageState::ArchiveMissing);
        let missing = std::fs::read_to_string(&path).expect("source page is readable");
        assert!(missing.starts_with(SOURCE_PAGE_MARKER));
        assert!(missing.contains("prnsd nnpages seed --source"));
        assert!(missing.contains("https://prns.dev"));
        assert!(missing.contains("https://reticulum.rs"));
        assert!(missing.contains("#!bg=000"));
        assert!(!missing.contains(":/file/source.zip"));

        assert_eq!(
            seed_source_page(directory.path()).expect("reseed succeeds"),
            SourcePageSeed::Unchanged(SourcePageState::ArchiveMissing)
        );

        let files = crate::nnpages::file_root(directory.path());
        std::fs::create_dir_all(&files).expect("file root");
        std::fs::write(
            files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME),
            vec![0u8; 2048],
        )
        .expect("archive");
        let SourcePageSeed::Written { path, state } =
            seed_source_page(directory.path()).expect("staged reseed succeeds")
        else {
            panic!("staging the archive rewrites the managed page");
        };
        assert_eq!(
            state,
            SourcePageState::ArchiveStaged {
                archive_bytes: 2048,
                has_checksum: false,
            }
        );
        let available = std::fs::read_to_string(&path).expect("source page is readable");
        assert!(available.starts_with(SOURCE_PAGE_MARKER));
        assert!(available.contains(":/file/source.zip"));
        assert!(available.contains("(2.0 KB)"));
        assert!(available.contains(crate::build_identity::VERSION));
        assert!(available.contains(crate::build_identity::COMMIT));
        assert!(available.contains("does not authenticate the node by itself"));
        assert!(!available.contains("source.zip.sha256"));
        assert!(!available.contains("{{"));

        std::fs::write(
            files.join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME),
            b"abc123",
        )
        .expect("checksum");
        let SourcePageSeed::Written { path, .. } =
            seed_source_page(directory.path()).expect("checksum reseed succeeds")
        else {
            panic!("a staged checksum rewrites the managed page");
        };
        assert!(std::fs::read_to_string(&path)
            .expect("source page is readable")
            .contains(":/file/source.zip.sha256"));
    }

    #[test]
    fn a_refresh_rerenders_the_managed_source_page_when_the_archive_appears() {
        let directory = tempfile::tempdir().expect("temporary directory");
        seed_source_page(directory.path()).expect("source page seeding succeeds");
        assert_eq!(
            refresh_source_page(directory.path()).expect("refresh succeeds"),
            SourcePageRefresh::Unchanged
        );

        let files = crate::nnpages::file_root(directory.path());
        std::fs::create_dir_all(&files).expect("file root");
        std::fs::write(
            files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME),
            vec![0u8; 2048],
        )
        .expect("archive");

        assert_eq!(
            refresh_source_page(directory.path()).expect("refresh succeeds"),
            SourcePageRefresh::Rewritten(SourcePageState::ArchiveStaged {
                archive_bytes: 2048,
                has_checksum: false,
            })
        );
        let page =
            crate::nnpages::page_root(directory.path()).join(crate::nnpages::SOURCE_PAGE_FILE_NAME);
        let rendered = std::fs::read_to_string(page).expect("source page is readable");
        assert!(rendered.contains(":/file/source.zip"));
        assert!(rendered.contains("(2.0 KB)"));
    }

    #[test]
    fn a_refresh_leaves_an_operator_owned_source_page_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let SourcePageSeed::Written { path, .. } =
            seed_source_page(directory.path()).expect("source page seeding succeeds")
        else {
            panic!("a fresh directory writes the source page");
        };
        std::fs::write(&path, b"operator edition").expect("operator edits page");
        assert_eq!(
            refresh_source_page(directory.path()).expect("refresh succeeds"),
            SourcePageRefresh::OperatorOwned
        );
        assert_eq!(
            std::fs::read(&path).expect("page is readable"),
            b"operator edition"
        );
    }

    #[test]
    fn a_refresh_without_a_source_page_touches_nothing() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            refresh_source_page(directory.path()).expect("refresh succeeds"),
            SourcePageRefresh::Absent
        );
        assert!(!crate::nnpages::root(directory.path()).exists());

        let SourcePageSeed::Written { path, .. } =
            seed_source_page(directory.path()).expect("source page seeding succeeds")
        else {
            panic!("a fresh directory writes the source page");
        };
        std::fs::remove_file(&path).expect("operator removes page");
        assert_eq!(
            refresh_source_page(directory.path()).expect("refresh succeeds"),
            SourcePageRefresh::Absent
        );
        assert!(!path.exists());
    }

    #[test]
    fn the_coming_from_rns_page_is_managed_until_edited() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let ManagedPageSeed::Written(path) =
            seed_coming_from_rns_page(directory.path()).expect("page seeding succeeds")
        else {
            panic!("a fresh directory writes the coming-from-rns page");
        };
        let content = std::fs::read_to_string(&path).expect("page is readable");
        assert!(content.starts_with(COMING_FROM_RNS_MARKER));
        assert!(content.contains("#!bg=000"));
        assert!(content.contains(":/page/source.mu"));
        assert!(content.contains("https://github.com/KenAKAFrosty/Prns"));
        assert!(content.contains(":/page/index.mu"));

        assert_eq!(
            seed_coming_from_rns_page(directory.path()).expect("reseed succeeds"),
            ManagedPageSeed::Unchanged
        );

        std::fs::write(&path, b"operator edition").expect("operator edits page");
        assert_eq!(
            seed_coming_from_rns_page(directory.path()).expect("reseed succeeds"),
            ManagedPageSeed::OperatorOwned
        );
        assert_eq!(
            std::fs::read(&path).expect("operator page is readable"),
            b"operator edition"
        );
    }

    #[test]
    fn an_operator_edited_source_page_is_never_rewritten() {
        let directory = tempfile::tempdir().expect("temporary directory");
        seed_source_page(directory.path()).expect("source page seeding succeeds");
        let path =
            crate::nnpages::page_root(directory.path()).join(crate::nnpages::SOURCE_PAGE_FILE_NAME);
        std::fs::write(&path, b"operator edition").expect("operator edits page");
        assert_eq!(
            seed_source_page(directory.path()).expect("reseed succeeds"),
            SourcePageSeed::OperatorOwned
        );
        assert_eq!(
            std::fs::read(&path).expect("operator page is readable"),
            b"operator edition"
        );
    }

    #[test]
    fn archive_sizes_format_for_humans() {
        assert_eq!(format_archive_size(999), "999 B");
        assert_eq!(format_archive_size(1024), "1.0 KB");
        assert_eq!(format_archive_size(2048), "2.0 KB");
        assert_eq!(format_archive_size(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
        assert_eq!(format_archive_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn source_archives_are_staged_once_with_a_verified_checksum() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("release.zip");
        std::fs::write(&source, b"exact release source").expect("source");
        let digest =
            data_encoding::HEXLOWER.encode(&personal_rns::crypto::sha256(b"exact release source"));
        std::fs::write(
            directory.path().join("release.zip.sha256"),
            format!("{digest}  release.zip\n"),
        )
        .expect("checksum");
        let config = directory.path().join("node");

        let staged = stage_source_archive(&config, &source).expect("stage source");
        assert_eq!(staged.archive_bytes, 20);
        assert_eq!(staged.created.len(), 3);
        assert_eq!(
            std::fs::read(&staged.archive_path).expect("staged archive"),
            b"exact release source"
        );
        assert_eq!(
            std::fs::read_to_string(
                crate::nnpages::file_root(&config).join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME)
            )
            .expect("staged checksum"),
            format!("{digest}  {}\n", crate::nnpages::SOURCE_ARCHIVE_FILE_NAME)
        );
        assert_eq!(
            std::fs::read(
                crate::nnpages::file_root(&config).join(SOURCE_ARCHIVE_RECEIPT_FILE_NAME)
            )
            .expect("management receipt"),
            source_archive_receipt(b"exact release source")
        );

        let repeated = stage_source_archive(&config, &source).expect("repeat stage");
        assert!(repeated.created.is_empty());
        assert!(repeated.replaced.is_empty());
    }

    #[test]
    fn a_managed_source_archive_advances_with_its_bundled_release() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("release.zip");
        let config = directory.path().join("node");
        std::fs::write(&source, b"release one").expect("first source");
        stage_source_archive(&config, &source).expect("stage first source");

        std::fs::write(&source, b"release two").expect("second source");
        let updated = stage_source_archive(&config, &source).expect("advance staged source");
        assert!(updated.created.is_empty());
        assert_eq!(updated.replaced.len(), 3);
        assert_eq!(
            std::fs::read(&updated.archive_path).expect("updated archive"),
            b"release two"
        );
        assert_eq!(
            std::fs::read(
                crate::nnpages::file_root(&config).join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME)
            )
            .expect("updated checksum"),
            source_checksum_document(b"release two")
        );
    }

    #[test]
    fn automatic_source_refresh_honors_the_hosting_opt_in_boundary() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            refresh_staged_bundled_source(directory.path()).expect("absent source is accepted"),
            BundledSourceRefresh::NotStaged
        );

        let files = crate::nnpages::file_root(directory.path());
        std::fs::create_dir_all(&files).expect("files");
        std::fs::write(
            files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME),
            b"operator archive",
        )
        .expect("operator archive");
        assert_eq!(
            refresh_staged_bundled_source(directory.path()).expect("operator source is preserved"),
            BundledSourceRefresh::OperatorOwned
        );

        std::fs::write(
            files.join(crate::nnpages::SOURCE_CHECKSUM_FILE_NAME),
            source_checksum_document(b"operator archive"),
        )
        .expect("operator checksum");
        assert_eq!(
            refresh_staged_bundled_source(directory.path())
                .expect("operator checksum pair is preserved"),
            BundledSourceRefresh::OperatorOwned
        );

        let adopted = stage_source_archive(
            directory.path(),
            &files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME),
        )
        .expect("explicit staging adopts a checksum pair");
        assert!(adopted
            .created
            .iter()
            .any(|path| path.ends_with(SOURCE_ARCHIVE_RECEIPT_FILE_NAME)));
    }

    #[test]
    fn source_staging_rejects_mismatched_checksums_and_operator_conflicts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("release.zip");
        std::fs::write(&source, b"source").expect("source");
        std::fs::write(
            directory.path().join("release.zip.sha256"),
            format!("{}  release.zip\n", "0".repeat(64)),
        )
        .expect("checksum");
        let config = directory.path().join("node");
        assert!(matches!(
            stage_source_archive(&config, &source),
            Err(ServerBootstrapError::SourceChecksumMismatch { .. })
        ));
        assert!(!crate::nnpages::file_root(&config).exists());

        std::fs::remove_file(directory.path().join("release.zip.sha256")).expect("checksum");
        let files = crate::nnpages::file_root(&config);
        std::fs::create_dir_all(&files).expect("files");
        let target = files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME);
        std::fs::write(&target, b"operator archive").expect("operator archive");
        assert!(matches!(
            stage_source_archive(&config, &source),
            Err(ServerBootstrapError::HostedFileConflict { .. })
        ));
        assert_eq!(
            std::fs::read(target).expect("operator archive remains"),
            b"operator archive"
        );
    }

    #[test]
    fn oversized_source_archives_are_not_advertised() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let files = crate::nnpages::file_root(directory.path());
        std::fs::create_dir_all(&files).expect("files");
        let archive =
            File::create(files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME)).expect("archive");
        archive
            .set_len(crate::nnpages::MAX_FILE_BYTES + 1)
            .expect("sparse archive");
        assert_eq!(
            source_archive_state(directory.path()),
            SourcePageState::ArchiveMissing
        );
    }

    #[test]
    fn bootstrap_rollback_removes_only_its_own_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = crate::nnpages::page_root(directory.path());
        std::fs::create_dir_all(&root).expect("pages");
        let source_page = root.join(crate::nnpages::SOURCE_PAGE_FILE_NAME);
        let coming_page = root.join(crate::nnpages::COMING_FROM_RNS_PAGE_FILE_NAME);
        std::fs::write(&source_page, b"operator source").expect("operator source");
        std::fs::write(&coming_page, b"operator guide").expect("operator guide");

        let mut transaction = NnPagesSeedTransaction::default();
        let settings = materialize_nnpages_settings(
            directory.path(),
            "announce = true\nannounce_interval_minutes = 360\n",
        )
        .expect("materialize settings")
        .expect("new settings");
        transaction.record(NnPagesSeedChange::Created(settings.clone()));
        let index = seed_default_page(directory.path())
            .expect("seed index")
            .expect("new index");
        transaction.record(NnPagesSeedChange::Created(index.clone()));
        transaction.record_optional(
            seed_source_page_tracked(directory.path())
                .expect("source")
                .change,
        );
        transaction.record_optional(
            seed_coming_from_rns_page_tracked(directory.path())
                .expect("guide")
                .change,
        );
        let error = transaction.rollback(ServerBootstrapError::InvalidNnPagesTarget {
            path: directory.path().join("config"),
        });
        assert!(matches!(
            error,
            ServerBootstrapError::InvalidNnPagesTarget { .. }
        ));
        assert!(!index.exists());
        assert!(!settings.exists());
        assert_eq!(
            std::fs::read(source_page).expect("operator source remains"),
            b"operator source"
        );
        assert_eq!(
            std::fs::read(coming_page).expect("operator guide remains"),
            b"operator guide"
        );
    }

    #[test]
    fn bootstrap_rollback_restores_a_managed_page_it_replaced() {
        let directory = tempfile::tempdir().expect("temporary directory");
        seed_source_page(directory.path()).expect("initial source page");
        let path =
            crate::nnpages::page_root(directory.path()).join(crate::nnpages::SOURCE_PAGE_FILE_NAME);
        let before = std::fs::read(&path).expect("before");
        let files = crate::nnpages::file_root(directory.path());
        std::fs::create_dir_all(&files).expect("files");
        std::fs::write(
            files.join(crate::nnpages::SOURCE_ARCHIVE_FILE_NAME),
            b"archive",
        )
        .expect("archive");

        let tracked = seed_source_page_tracked(directory.path()).expect("rewrite source page");
        assert!(matches!(tracked.outcome, SourcePageSeed::Written { .. }));
        let mut transaction = NnPagesSeedTransaction::default();
        transaction.record_optional(tracked.change);
        let _ = transaction.rollback(ServerBootstrapError::InvalidNnPagesTarget {
            path: directory.path().join("config"),
        });
        assert_eq!(std::fs::read(path).expect("restored"), before);
    }
}
