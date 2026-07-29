use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use personal_rns::config::editing::{ConfigEdit, ConfigEditError, ConfigFile, ConfigFileError};
use personal_rns::config::{discover, DiscoveryError};

const CONFIG_FILE_NAME: &str = "config";
const DEFAULT_BACKBONE_PORT: u16 = 4242;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const LISTEN_PORT: &str = "PRNSD_BACKBONE_LISTEN_PORT";
const REACHABLE_HOST: &str = "PRNSD_REACHABLE_HOST";
const REACHABLE_PORT: &str = "PRNSD_REACHABLE_PORT";
const RAILWAY_HOST: &str = "RAILWAY_TCP_PROXY_DOMAIN";
const RAILWAY_PORT: &str = "RAILWAY_TCP_PROXY_PORT";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedEndpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerBootstrapEnvironment {
    listen_port: u16,
    published: Option<PublishedEndpoint>,
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
        Ok(Self {
            listen_port,
            published,
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
            self.listen_port
        );
        match &self.published {
            Some(endpoint) => {
                config.push_str(&format!(
                    "discoverable = Yes\n\
                     reachable_on = {}\n\
                     reachable_port = {}\n",
                    endpoint.host, endpoint.port
                ));
            }
            None => config.push_str("discoverable = No\n"),
        }
        config
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
    let page = seed_default_page(&discovered.dir)?;
    if let Err(error) = materialize(&path, &environment.render()) {
        if page.is_some() && !path.exists() {
            cleanup_seeded_page(&discovered.dir);
        }
        return Err(error);
    }
    Ok(Some(ServerBootstrapReceipt {
        config_path: path,
        seeded_page: page,
    }))
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

fn seed_default_page(config_dir: &Path) -> Result<Option<PathBuf>, ServerBootstrapError> {
    let root = crate::node_pages::page_root(config_dir);
    prepare_page_directory(&root)?;
    let path = root.join(crate::node_pages::INDEX_FILE_NAME);
    if path.exists() {
        validate_page_target(&path)?;
        return Ok(None);
    }

    let staging = create_staging_file(&root)?;
    let staging_path = staging.1;
    let mut file = staging.0;
    let result = (|| {
        file.write_all(crate::node_pages::DEFAULT_INDEX_PAGE)
            .map_err(|source| page_storage("write staging page", &staging_path, source))?;
        file.sync_all()
            .map_err(|source| page_storage("sync staging page", &staging_path, source))?;
        drop(file);
        match fs::hard_link(&staging_path, &path) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                validate_page_target(&path)?;
                return Ok(None);
            }
            Err(source) => {
                return Err(page_storage("publish page", &path, source));
            }
        }
        sync_page_directory(&root)?;
        Ok(Some(path.clone()))
    })();
    let _ = fs::remove_file(&staging_path);
    result
}

fn prepare_page_directory(root: &Path) -> Result<(), ServerBootstrapError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            return Err(ServerBootstrapError::InvalidPageTarget {
                path: root.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(page_storage("inspect page directory", root, source)),
    }
    fs::create_dir_all(root)
        .map_err(|source| page_storage("create page directory", root, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|source| page_storage("protect page directory", root, source))?;
    }
    Ok(())
}

fn validate_page_target(path: &Path) -> Result<(), ServerBootstrapError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| page_storage("inspect existing page", path, source))?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(ServerBootstrapError::InvalidPageTarget {
            path: path.to_path_buf(),
        })
    }
}

fn create_staging_file(root: &Path) -> Result<(File, PathBuf), ServerBootstrapError> {
    for _ in 0..64 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".{}.tmp-{}-{sequence}",
            crate::node_pages::INDEX_FILE_NAME,
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
            Err(source) => return Err(page_storage("create staging page", &path, source)),
        }
    }
    Err(page_storage(
        "create staging page",
        root,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique staging filename",
        ),
    ))
}

#[cfg(unix)]
fn sync_page_directory(root: &Path) -> Result<(), ServerBootstrapError> {
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| page_storage("sync page directory", root, source))
}

#[cfg(not(unix))]
fn sync_page_directory(_root: &Path) -> Result<(), ServerBootstrapError> {
    Ok(())
}

fn cleanup_seeded_page(config_dir: &Path) {
    let root = crate::node_pages::page_root(config_dir);
    let _ = fs::remove_file(root.join(crate::node_pages::INDEX_FILE_NAME));
    let _ = fs::remove_dir(root);
}

fn page_storage(operation: &'static str, path: &Path, source: io::Error) -> ServerBootstrapError {
    ServerBootstrapError::PageStorage {
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

#[derive(Debug)]
pub(super) enum ServerBootstrapError {
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
    ConfigFile(ConfigFileError),
    ConfigEdit(ConfigEditError),
    PageStorage {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    InvalidPageTarget {
        path: PathBuf,
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
            Self::ConfigFile(error) => error.fmt(formatter),
            Self::ConfigEdit(error) => error.fmt(formatter),
            Self::PageStorage {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {} failed: {source}", path.display()),
            Self::InvalidPageTarget { path } => write!(
                formatter,
                "server page target {} is not a regular file or directory",
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
            Self::PageStorage { source, .. } => Some(source),
            Self::NonUtf8 { .. }
            | Self::IncompleteEndpoint { .. }
            | Self::InvalidHost { .. }
            | Self::InvalidPort { .. }
            | Self::InvalidPageTarget { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use personal_rns::config::{parse_and_plan, DiscoveryAdvertisementPlan};

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
    }

    #[test]
    fn materialization_is_owner_only_and_never_rewrites_existing_config() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(CONFIG_FILE_NAME);
        materialize(
            &path,
            &ServerBootstrapEnvironment {
                listen_port: 4242,
                published: None,
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
    fn default_page_is_seeded_once_and_remains_operator_owned() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let page = seed_default_page(directory.path())
            .expect("page seeding succeeds")
            .expect("page is newly seeded");
        assert_eq!(
            std::fs::read(&page).expect("seeded page is readable"),
            crate::node_pages::DEFAULT_INDEX_PAGE
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
                published: None,
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
        let root = crate::node_pages::page_root(directory.path());
        std::fs::create_dir(&root).expect("page root");
        std::fs::create_dir(root.join(crate::node_pages::INDEX_FILE_NAME))
            .expect("invalid page directory");

        assert!(matches!(
            seed_default_page(directory.path()),
            Err(ServerBootstrapError::InvalidPageTarget { .. })
        ));
    }
}
