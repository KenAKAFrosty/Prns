use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use personal_rns::config::{
    discover, parse_and_plan_named, ConfigErrors, ConfigReport, DaemonPlan, DiscoveredConfig,
    DiscoveryError, SharedInstance, SharedInstanceTransport as ConfigSharedInstanceTransport,
};
use personal_rns::identity::vault::{read_identity_file, FileVaultError};
use personal_rns::identity::{in_memory::InMemoryNodeIdentity, IdentityHash, IdentitySigner};
use personal_rns::interfaces::shared_instance::rns_rpc::RpcAuthenticationKey;
use personal_rns::interfaces::{BitrateBps, ConfiguredInterfacePolicy};
use personal_rns::shared_instance::{
    SharedInstanceClientIntent, SharedInstancePorts, SharedInstanceRpcClient,
    SharedInstanceRpcEndpoint, SharedInstanceTransport as RuntimeSharedInstanceTransport,
};

pub struct LoadedConfiguration {
    pub discovered: DiscoveredConfig,
    pub report: ConfigReport<DaemonPlan>,
}

impl LoadedConfiguration {
    pub fn load(config_dir: Option<&Path>) -> Result<Self, UtilityConfigurationError> {
        let discovered = discover(config_dir).map_err(UtilityConfigurationError::Discovery)?;
        let (source, text) = match &discovered.config {
            Some(path) => (
                path.display().to_string(),
                std::fs::read_to_string(path).map_err(|source| {
                    UtilityConfigurationError::Read {
                        path: path.clone(),
                        source,
                    }
                })?,
            ),
            None => (
                String::from("<built-in config>"),
                crate::daemon::DEFAULT_CONFIG.to_owned(),
            ),
        };
        let report = parse_and_plan_named(source, &text)
            .map_err(UtilityConfigurationError::InvalidConfig)?;
        Ok(Self { discovered, report })
    }

    pub fn local_rpc_client(
        &self,
        timeout: Duration,
    ) -> Result<SharedInstanceRpcClient, UtilityConfigurationError> {
        let shared = self.shared_instance()?;
        let endpoint = rpc_endpoint(shared.name, shared.transport, shared.ports.control);
        let rpc_key = match shared.rpc_key {
            Some(rpc_key) => RpcAuthenticationKey::new(rpc_key.to_vec()),
            None => {
                let secret = self.transport_identity_secret()?;
                RpcAuthenticationKey::from_rns_transport_identity_secret(&secret)
            }
        };
        Ok(SharedInstanceRpcClient::new(endpoint, rpc_key, timeout))
    }

    pub fn local_bus_client_intent(
        &self,
    ) -> Result<SharedInstanceClientIntent, UtilityConfigurationError> {
        let shared = self.shared_instance()?;
        Ok(SharedInstanceClientIntent {
            bus_port: shared.ports.bus,
            transport: runtime_transport(shared.name, shared.transport),
            policy: personal_rns::interfaces::shared_instance::configured_policy(
                ConfiguredInterfacePolicy {
                    bitrate: shared.forced_bitrate,
                    ..Default::default()
                },
            ),
        })
    }

    pub fn local_transport_identity_hash(&self) -> Result<IdentityHash, UtilityConfigurationError> {
        self.transport_identity_secret()
            .map(|secret| InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash())
    }

    fn transport_identity_secret(
        &self,
    ) -> Result<personal_rns::identity::vault::IdentitySecretKey, UtilityConfigurationError> {
        let path = self
            .discovered
            .dir
            .join("storage")
            .join("transport_identity");
        read_identity_file(&path)
            .map_err(|source| UtilityConfigurationError::Identity {
                path: path.clone(),
                source,
            })?
            .ok_or(UtilityConfigurationError::IdentityMissing { path })
    }

    fn shared_instance(&self) -> Result<SharedInstanceSettings<'_>, UtilityConfigurationError> {
        let SharedInstance::Enabled {
            name,
            transport,
            instance_port,
            control_port,
            rpc_key,
            forced_bitrate,
        } = &self.report.value.shared_instance
        else {
            return Err(UtilityConfigurationError::SharedInstanceDisabled);
        };
        Ok(SharedInstanceSettings {
            name,
            transport: *transport,
            ports: SharedInstancePorts {
                bus: *instance_port,
                control: *control_port,
            },
            rpc_key: rpc_key.as_deref(),
            forced_bitrate: *forced_bitrate,
        })
    }
}

struct SharedInstanceSettings<'a> {
    name: &'a str,
    transport: ConfigSharedInstanceTransport,
    ports: SharedInstancePorts,
    rpc_key: Option<&'a [u8]>,
    forced_bitrate: Option<BitrateBps>,
}

fn rpc_endpoint(
    name: &str,
    transport: ConfigSharedInstanceTransport,
    control_port: u16,
) -> SharedInstanceRpcEndpoint {
    match transport {
        ConfigSharedInstanceTransport::Tcp => SharedInstanceRpcEndpoint::tcp(control_port),
        ConfigSharedInstanceTransport::Unix => {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                SharedInstanceRpcEndpoint::abstract_unix(name)
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                let _ = name;
                SharedInstanceRpcEndpoint::tcp(control_port)
            }
        }
    }
}

fn runtime_transport(
    name: &str,
    transport: ConfigSharedInstanceTransport,
) -> RuntimeSharedInstanceTransport {
    match transport {
        ConfigSharedInstanceTransport::Tcp => RuntimeSharedInstanceTransport::Tcp,
        ConfigSharedInstanceTransport::Unix => {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                RuntimeSharedInstanceTransport::AbstractUnix {
                    socket_path: name.to_owned(),
                }
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                let _ = name;
                RuntimeSharedInstanceTransport::Tcp
            }
        }
    }
}

#[derive(Debug)]
pub enum UtilityConfigurationError {
    Discovery(DiscoveryError),
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidConfig(ConfigErrors),
    SharedInstanceDisabled,
    IdentityMissing {
        path: PathBuf,
    },
    Identity {
        path: PathBuf,
        source: FileVaultError,
    },
}

impl fmt::Display for UtilityConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(error) => write!(formatter, "config discovery failed: {error}"),
            Self::Read { path, source } => {
                write!(formatter, "could not read config {}: {source}", path.display())
            }
            Self::InvalidConfig(errors) => errors.fmt(formatter),
            Self::SharedInstanceDisabled => formatter.write_str(
                "the selected config has share_instance = No; utilities need a running shared RNS instance",
            ),
            Self::IdentityMissing { path } => write!(
                formatter,
                "shared-instance RPC credentials are unavailable: {} does not exist; start prnsd first or configure [reticulum] rpc_key",
                path.display()
            ),
            Self::Identity { path, source } => write!(
                formatter,
                "could not read shared-instance identity {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for UtilityConfigurationError {}
