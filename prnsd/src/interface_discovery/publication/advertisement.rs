#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use personal_rns::config::{
    DiscoveryAdvertisementPlan, DiscoveryAnnouncementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, InterfaceAccessPlan,
};
use personal_rns::interface_discovery::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveryAdvertisement,
    DiscoveryPublicationRegistration, DiscoveryPublicationSecurity, GeographicLocation,
    PublishedIfac,
};
use personal_rns::interfaces::InterfaceId;
#[cfg(unix)]
use personal_rns::interfaces::InterfaceOriginKind;
use personal_rns::wire::TransportId;

pub(super) struct PublicationSource {
    pub(super) interface_name: String,
    access: InterfaceAccessPlan,
    pub(super) announcement: DiscoveryAnnouncementPlan,
    transport_enabled: bool,
    transport_id: TransportId,
}

impl PublicationSource {
    pub(super) fn new(
        interface_name: String,
        access: InterfaceAccessPlan,
        announcement: DiscoveryAnnouncementPlan,
        transport_enabled: bool,
        transport_id: TransportId,
    ) -> Self {
        Self {
            interface_name,
            access,
            announcement,
            transport_enabled,
            transport_id,
        }
    }

    pub(super) fn registration(&self, interface: InterfaceId) -> DiscoveryPublicationRegistration {
        DiscoveryPublicationRegistration {
            interface,
            interval: self.announcement.interval,
            stamp_cost: self.announcement.stamp_cost,
            security: self.security(),
        }
    }

    pub(super) fn security(&self) -> DiscoveryPublicationSecurity {
        match self.announcement.encryption {
            DiscoveryEncryption::Plaintext => DiscoveryPublicationSecurity::Plaintext,
            DiscoveryEncryption::NetworkIdentity => DiscoveryPublicationSecurity::NetworkEncrypted,
        }
    }

    pub(super) fn interface_type(&self) -> AdvertisedInterfaceType {
        match self.announcement.advertisement {
            DiscoveryAdvertisementPlan::Backbone { .. } => AdvertisedInterfaceType::Backbone,
            DiscoveryAdvertisementPlan::TcpServer { .. } => AdvertisedInterfaceType::TcpServer,
            DiscoveryAdvertisementPlan::RNode { .. } => AdvertisedInterfaceType::RNode,
            DiscoveryAdvertisementPlan::Kiss { .. } => AdvertisedInterfaceType::Kiss,
        }
    }

    pub(super) async fn advertisement(
        &self,
        interface: InterfaceId,
    ) -> Result<DiscoveryAdvertisement, DiscoveryAdvertisementResolutionError> {
        let details = match &self.announcement.advertisement {
            DiscoveryAdvertisementPlan::Backbone { reachable_on, port }
            | DiscoveryAdvertisementPlan::TcpServer { reachable_on, port } => {
                AdvertisementDetails::Reachable {
                    host: resolve_reachable_on(reachable_on, interface, &self.interface_name)
                        .await?,
                    port: *port,
                }
            }
            DiscoveryAdvertisementPlan::RNode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
            } => AdvertisementDetails::RNode {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                spreading_factor: *spreading_factor,
                coding_rate: *coding_rate,
            },
            DiscoveryAdvertisementPlan::Kiss {
                frequency_hz,
                bandwidth_hz,
                modulation,
            } => AdvertisementDetails::Kiss {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                modulation: sanitize(modulation),
            },
        };
        let published_ifac = match self.announcement.ifac {
            DiscoveryIfacPublication::Omit => None,
            DiscoveryIfacPublication::Include => {
                let (network_name, passphrase) = match &self.access {
                    InterfaceAccessPlan::Open => (None, None),
                    InterfaceAccessPlan::Ifac {
                        network_name,
                        passphrase,
                        ..
                    } => (
                        network_name.as_deref().map(sanitize),
                        passphrase.as_deref().map(sanitize),
                    ),
                };
                Some(PublishedIfac {
                    network_name,
                    passphrase,
                })
            }
        };
        Ok(DiscoveryAdvertisement {
            interface_type: self.interface_type(),
            transport: AdvertisedTransport::from_wire(self.transport_enabled, self.transport_id),
            name: self.announcement.name.as_deref().map(sanitize),
            location: GeographicLocation {
                latitude: self.announcement.location.latitude,
                longitude: self.announcement.location.longitude,
                height: self.announcement.location.height,
            },
            details,
            published_ifac,
        })
    }
}

pub(super) async fn resolve_reachable_on(
    configured: &str,
    interface: InterfaceId,
    interface_name: &str,
) -> Result<String, DiscoveryAdvertisementResolutionError> {
    let reachable_on = sanitize(configured);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let configured_path = Path::new(&reachable_on);
        let path = expand_user_path(
            configured_path,
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .as_deref(),
        )?;
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(_) => return Ok(reachable_on),
        };
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Ok(reachable_on);
        }
        tracing::debug!(
            event = "interface_discovery_reachable_on_evaluating",
            interface_origin = InterfaceOriginKind::Configured.as_str(),
            interface = ?interface.as_bytes(),
            interface_name,
            path = %path.display(),
        );
        let output = tokio::process::Command::new(&path)
            .output()
            .await
            .map_err(|source| DiscoveryAdvertisementResolutionError::Execute {
                path: path.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(DiscoveryAdvertisementResolutionError::Exit {
                path,
                status: output.status,
            });
        }
        let stdout = String::from_utf8(output.stdout).map_err(|source| {
            DiscoveryAdvertisementResolutionError::OutputNotUtf8 {
                path: path.clone(),
                source,
            }
        })?;
        Ok(sanitize(&stdout))
    }
    #[cfg(not(unix))]
    {
        let _ = (interface, interface_name);
        Ok(reachable_on)
    }
}

#[cfg(unix)]
pub(super) fn expand_user_path(
    path: &Path,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, DiscoveryAdvertisementResolutionError> {
    let Ok(rest) = path.strip_prefix("~") else {
        return Ok(path.to_path_buf());
    };
    let Some(home) = home else {
        return Err(DiscoveryAdvertisementResolutionError::HomeUnavailable {
            path: path.to_path_buf(),
        });
    };
    Ok(Path::new(home).join(rest))
}

fn sanitize(value: &str) -> String {
    value.replace(['\n', '\r'], "").trim().to_string()
}

pub(super) fn security_name(security: DiscoveryPublicationSecurity) -> &'static str {
    match security {
        DiscoveryPublicationSecurity::Plaintext => "plaintext",
        DiscoveryPublicationSecurity::NetworkEncrypted => "network_encrypted",
    }
}

#[derive(Debug)]
pub(super) enum DiscoveryAdvertisementResolutionError {
    UnknownInterface {
        interface: InterfaceId,
    },
    HomeUnavailable {
        path: PathBuf,
    },
    Execute {
        path: PathBuf,
        source: std::io::Error,
    },
    Exit {
        path: PathBuf,
        status: std::process::ExitStatus,
    },
    OutputNotUtf8 {
        path: PathBuf,
        source: std::string::FromUtf8Error,
    },
}

impl core::fmt::Display for DiscoveryAdvertisementResolutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownInterface { interface } => {
                write!(formatter, "unknown discovery interface {interface:?}")
            }
            Self::HomeUnavailable { path } => write!(
                formatter,
                "reachable_on path {} needs a home directory, but HOME is unavailable",
                path.display()
            ),
            Self::Execute { path, source } => {
                write!(formatter, "could not execute {}: {source}", path.display())
            }
            Self::Exit { path, status } => {
                write!(formatter, "{} exited with {status}", path.display())
            }
            Self::OutputNotUtf8 { path, source } => {
                write!(
                    formatter,
                    "{} returned non-UTF-8 output: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for DiscoveryAdvertisementResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Execute { source, .. } => Some(source),
            Self::OutputNotUtf8 { source, .. } => Some(source),
            Self::UnknownInterface { .. } | Self::HomeUnavailable { .. } | Self::Exit { .. } => {
                None
            }
        }
    }
}
