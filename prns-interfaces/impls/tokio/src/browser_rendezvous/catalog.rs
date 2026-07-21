use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::{Host, Url};

use prns_core::interfaces::browser_rendezvous as contract;
use prns_core::interfaces::browser_rendezvous::BrowserRendezvousId;

const CATALOG_VERSION: u8 = 1;
const MAX_CATALOG_GATEWAYS: usize = 64;
pub(super) const MAX_CATALOG_BODY_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserGatewayEndpoint {
    id: BrowserRendezvousId,
    target: BrowserGatewayTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowserGatewayTarget {
    Address(SocketAddr),
    LocalHostname(String),
}

impl BrowserGatewayEndpoint {
    pub fn new(
        id: BrowserRendezvousId,
        address: SocketAddr,
    ) -> Result<Self, BrowserGatewayEndpointError> {
        if address.port() != contract::PORT {
            return Err(BrowserGatewayEndpointError::Port(address.port()));
        }
        if !contract::is_local_address(address.ip()) {
            return Err(BrowserGatewayEndpointError::Address(address));
        }
        Ok(Self {
            id,
            target: BrowserGatewayTarget::Address(address),
        })
    }

    pub fn from_local_hostname(
        id: BrowserRendezvousId,
        hostname: impl Into<String>,
    ) -> Result<Self, BrowserGatewayEndpointError> {
        let hostname = hostname.into();
        let hostname = hostname
            .strip_suffix('.')
            .unwrap_or(&hostname)
            .to_ascii_lowercase();
        if !is_local_hostname(&hostname) {
            return Err(BrowserGatewayEndpointError::Hostname(hostname));
        }
        Ok(Self {
            id,
            target: BrowserGatewayTarget::LocalHostname(hostname),
        })
    }

    pub const fn id(&self) -> BrowserRendezvousId {
        self.id
    }

    pub const fn address(&self) -> Option<SocketAddr> {
        match &self.target {
            BrowserGatewayTarget::Address(address) => Some(*address),
            BrowserGatewayTarget::LocalHostname(_) => None,
        }
    }

    pub fn hostname(&self) -> Option<&str> {
        match &self.target {
            BrowserGatewayTarget::Address(_) => None,
            BrowserGatewayTarget::LocalHostname(hostname) => Some(hostname),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserGatewayEndpointError {
    Port(u16),
    Address(SocketAddr),
    Hostname(String),
}

impl std::fmt::Display for BrowserGatewayEndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Port(port) => write!(
                formatter,
                "browser gateway uses port {port}, not {}",
                contract::PORT
            ),
            Self::Address(address) => {
                write!(formatter, "browser gateway address {address} is not local")
            }
            Self::Hostname(hostname) => {
                write!(
                    formatter,
                    "browser gateway hostname {hostname} is not local"
                )
            }
        }
    }
}

impl std::error::Error for BrowserGatewayEndpointError {}

#[derive(Clone)]
pub(super) struct Catalog {
    shared: Arc<RwLock<CatalogEntries>>,
}

struct CatalogEntries {
    own_id: BrowserRendezvousId,
    lan_discovery: BTreeMap<BrowserRendezvousId, BrowserGatewayTarget>,
    injected: BTreeMap<BrowserRendezvousId, BrowserGatewayTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogSource {
    LanDiscovery,
    Injected,
}

impl Catalog {
    pub(super) fn new(own_id: BrowserRendezvousId) -> Self {
        Self {
            shared: Arc::new(RwLock::new(CatalogEntries {
                own_id,
                lan_discovery: BTreeMap::new(),
                injected: BTreeMap::new(),
            })),
        }
    }

    pub(super) fn replace_discovered(
        &self,
        source: CatalogSource,
        endpoints: impl IntoIterator<Item = BrowserGatewayEndpoint>,
    ) {
        let Ok(mut entries) = self.shared.write() else {
            return;
        };
        let own_id = entries.own_id;
        let discovered = match source {
            CatalogSource::LanDiscovery => &mut entries.lan_discovery,
            CatalogSource::Injected => &mut entries.injected,
        };
        discovered.clear();
        for endpoint in endpoints.into_iter().take(MAX_CATALOG_GATEWAYS) {
            if endpoint.id == own_id {
                continue;
            }
            discovered.entry(endpoint.id).or_insert(endpoint.target);
        }
    }

    pub(super) fn render(&self, local: SocketAddr) -> Result<Vec<u8>, CatalogRenderError> {
        let entries = self
            .shared
            .read()
            .map_err(|_| CatalogRenderError::Unavailable)?;
        let own = BrowserGatewayEndpoint::new(entries.own_id, local)
            .map_err(CatalogRenderError::Endpoint)?;
        let mut discovered = BTreeMap::new();
        for source in [&entries.lan_discovery, &entries.injected] {
            for (id, target) in source {
                if discovered.len() == MAX_CATALOG_GATEWAYS - 1 {
                    break;
                }
                discovered.entry(*id).or_insert_with(|| target.clone());
            }
        }
        let endpoints = std::iter::once(own).chain(
            discovered
                .into_iter()
                .map(|(id, target)| BrowserGatewayEndpoint { id, target }),
        );
        let catalog = CatalogV1::from_endpoints(endpoints);
        let body = serde_json::to_vec(&catalog).map_err(CatalogRenderError::Serialize)?;
        if body.len() > MAX_CATALOG_BODY_LEN {
            return Err(CatalogRenderError::TooLarge { actual: body.len() });
        }
        Ok(body)
    }
}

#[derive(Debug)]
pub(super) enum CatalogRenderError {
    Unavailable,
    Endpoint(BrowserGatewayEndpointError),
    Serialize(serde_json::Error),
    TooLarge { actual: usize },
}

impl std::fmt::Display for CatalogRenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("browser gateway catalog is unavailable"),
            Self::Endpoint(error) => error.fmt(formatter),
            Self::Serialize(error) => write!(formatter, "browser gateway catalog JSON: {error}"),
            Self::TooLarge { actual } => write!(
                formatter,
                "browser gateway catalog is {actual} bytes; maximum is {MAX_CATALOG_BODY_LEN}"
            ),
        }
    }
}

impl std::error::Error for CatalogRenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Endpoint(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Unavailable | Self::TooLarge { .. } => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CatalogV1 {
    #[serde(deserialize_with = "deserialize_catalog_version")]
    version: u8,
    gateways: Vec<CatalogGatewayV1>,
}

impl CatalogV1 {
    fn from_endpoints(endpoints: impl IntoIterator<Item = BrowserGatewayEndpoint>) -> Self {
        Self {
            version: CATALOG_VERSION,
            gateways: endpoints
                .into_iter()
                .take(MAX_CATALOG_GATEWAYS)
                .map(CatalogGatewayV1::from)
                .collect(),
        }
    }

    fn decode(
        bytes: &[u8],
    ) -> Result<Vec<BrowserGatewayEndpoint>, BrowserGatewayCatalogDecodeError> {
        if bytes.len() > MAX_CATALOG_BODY_LEN {
            return Err(BrowserGatewayCatalogDecodeError::BodyTooLarge {
                actual: bytes.len(),
            });
        }
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(BrowserGatewayCatalogDecodeError::Json)?;
        if catalog.gateways.len() > MAX_CATALOG_GATEWAYS {
            return Err(BrowserGatewayCatalogDecodeError::TooManyGateways {
                actual: catalog.gateways.len(),
            });
        }
        let mut ids = BTreeSet::new();
        let mut endpoints = Vec::with_capacity(catalog.gateways.len());
        for gateway in catalog.gateways {
            if !ids.insert(gateway.id.0) {
                return Err(BrowserGatewayCatalogDecodeError::DuplicateId(gateway.id.0));
            }
            endpoints.push(BrowserGatewayEndpoint {
                id: gateway.id.0,
                target: gateway.url.target,
            });
        }
        Ok(endpoints)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CatalogGatewayV1 {
    id: CatalogRendezvousId,
    url: CatalogUrl,
}

impl From<BrowserGatewayEndpoint> for CatalogGatewayV1 {
    fn from(endpoint: BrowserGatewayEndpoint) -> Self {
        Self {
            id: CatalogRendezvousId(endpoint.id),
            url: CatalogUrl {
                target: endpoint.target,
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CatalogRendezvousId(BrowserRendezvousId);

impl Serialize for CatalogRendezvousId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for CatalogRendezvousId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        BrowserRendezvousId::from_lower_hex(&value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CatalogUrl {
    target: BrowserGatewayTarget,
}

impl Serialize for CatalogUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let authority = match &self.target {
            BrowserGatewayTarget::Address(address) => address.to_string(),
            BrowserGatewayTarget::LocalHostname(hostname) => {
                format!("{hostname}:{}", contract::PORT)
            }
        };
        serializer.serialize_str(&format!("ws://{authority}{}", contract::PATH))
    }
}

impl<'de> Deserialize<'de> for CatalogUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let url = Url::parse(&value).map_err(serde::de::Error::custom)?;
        if url.scheme() != "ws"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port() != Some(contract::PORT)
            || url.path() != contract::PATH
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(serde::de::Error::custom(
                "invalid browser gateway URL shape",
            ));
        }
        let target = match url.host() {
            Some(Host::Ipv4(address)) => {
                let ip = IpAddr::V4(address);
                if !contract::is_local_address(ip) {
                    return Err(serde::de::Error::custom(
                        "browser gateway catalog URL address is not local",
                    ));
                }
                BrowserGatewayTarget::Address(SocketAddr::new(ip, contract::PORT))
            }
            Some(Host::Ipv6(address)) => {
                let ip = IpAddr::V6(address);
                if !contract::is_local_address(ip) {
                    return Err(serde::de::Error::custom(
                        "browser gateway catalog URL address is not local",
                    ));
                }
                BrowserGatewayTarget::Address(SocketAddr::new(ip, contract::PORT))
            }
            Some(Host::Domain(hostname)) if is_local_hostname(hostname) => {
                BrowserGatewayTarget::LocalHostname(
                    hostname
                        .strip_suffix('.')
                        .unwrap_or(hostname)
                        .to_ascii_lowercase(),
                )
            }
            Some(Host::Domain(_)) | None => {
                return Err(serde::de::Error::custom(
                    "browser gateway catalog URL hostname is not local",
                ));
            }
        };
        Ok(Self { target })
    }
}

fn is_local_hostname(hostname: &str) -> bool {
    let normalized = hostname.strip_suffix('.').unwrap_or(hostname);
    normalized.is_ascii()
        && normalized == normalized.to_ascii_lowercase()
        && normalized.len() <= 253
        && normalized.len() > ".local".len()
        && normalized.ends_with(".local")
        && !normalized.contains("..")
        && normalized.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn deserialize_catalog_version<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u8::deserialize(deserializer)?;
    if version == CATALOG_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported browser gateway catalog version {version}"
        )))
    }
}

#[derive(Debug)]
pub enum BrowserGatewayCatalogDecodeError {
    BodyTooLarge { actual: usize },
    Json(serde_json::Error),
    TooManyGateways { actual: usize },
    DuplicateId(BrowserRendezvousId),
}

impl std::fmt::Display for BrowserGatewayCatalogDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BodyTooLarge { actual } => write!(
                formatter,
                "browser gateway catalog body is {actual} bytes; maximum is {MAX_CATALOG_BODY_LEN}"
            ),
            Self::Json(error) => write!(formatter, "invalid browser gateway catalog JSON: {error}"),
            Self::TooManyGateways { actual } => write!(
                formatter,
                "browser gateway catalog has {actual} gateways; maximum is {MAX_CATALOG_GATEWAYS}"
            ),
            Self::DuplicateId(id) => write!(formatter, "browser gateway catalog repeats ID {id}"),
        }
    }
}

impl std::error::Error for BrowserGatewayCatalogDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::TooManyGateways { .. } | Self::DuplicateId(_) => None,
        }
    }
}

pub fn decode_browser_gateway_catalog(
    bytes: &[u8],
) -> Result<Vec<BrowserGatewayEndpoint>, BrowserGatewayCatalogDecodeError> {
    CatalogV1::decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_round_trip_through_the_typed_schema() {
        let own = BrowserRendezvousId::new([1; contract::ID_LEN]);
        let remote = BrowserRendezvousId::new([2; contract::ID_LEN]);
        let catalog = Catalog::new(own);
        catalog.replace_discovered(
            CatalogSource::LanDiscovery,
            [BrowserGatewayEndpoint::new(remote, "192.168.4.9:42721".parse().unwrap()).unwrap()],
        );

        let body = catalog.render("127.0.0.1:42721".parse().unwrap()).unwrap();
        assert_eq!(
            body,
            concat!(
                "{\"version\":1,\"gateways\":[",
                "{\"id\":\"01010101010101010101010101010101\",\"url\":\"ws://127.0.0.1:42721/prns\"},",
                "{\"id\":\"02020202020202020202020202020202\",\"url\":\"ws://192.168.4.9:42721/prns\"}",
                "]}"
            )
            .as_bytes()
        );
        assert_eq!(
            CatalogV1::decode(&body).unwrap(),
            vec![
                BrowserGatewayEndpoint::new(own, "127.0.0.1:42721".parse().unwrap()).unwrap(),
                BrowserGatewayEndpoint::new(remote, "192.168.4.9:42721".parse().unwrap()).unwrap(),
            ]
        );
    }

    #[test]
    fn hostile_catalog_shapes_fail_during_deserialization() {
        let public = br#"{"version":1,"gateways":[{"id":"01010101010101010101010101010101","url":"ws://8.8.8.8:42721/prns"}]}"#;
        let credentials = br#"{"version":1,"gateways":[{"id":"01010101010101010101010101010101","url":"ws://user@127.0.0.1:42721/prns"}]}"#;
        let unknown = br#"{"version":1,"gateways":[],"extra":true}"#;
        let uppercase = br#"{"version":1,"gateways":[{"id":"0101010101010101010101010101010A","url":"ws://127.0.0.1:42721/prns"}]}"#;

        for body in [
            public.as_slice(),
            credentials.as_slice(),
            unknown.as_slice(),
            uppercase.as_slice(),
        ] {
            assert!(CatalogV1::decode(body).is_err());
        }
    }

    #[test]
    fn duplicate_gateway_ids_are_rejected() {
        let body = br#"{"version":1,"gateways":[{"id":"01010101010101010101010101010101","url":"ws://127.0.0.1:42721/prns"},{"id":"01010101010101010101010101010101","url":"ws://192.168.1.2:42721/prns"}]}"#;
        assert!(matches!(
            CatalogV1::decode(body),
            Err(BrowserGatewayCatalogDecodeError::DuplicateId(_))
        ));
    }

    #[test]
    fn ipv6_catalog_urls_use_uri_brackets() {
        let own = BrowserRendezvousId::new([3; contract::ID_LEN]);
        let body = Catalog::new(own)
            .render("[fd00::1]:42721".parse().unwrap())
            .unwrap();
        assert!(std::str::from_utf8(&body)
            .unwrap()
            .contains("ws://[fd00::1]:42721/prns"));
    }

    #[test]
    fn independent_discovery_sources_are_merged_and_expire_independently() {
        let own = BrowserRendezvousId::new([1; contract::ID_LEN]);
        let lan = BrowserGatewayEndpoint::new(
            BrowserRendezvousId::new([2; contract::ID_LEN]),
            "192.168.4.2:42721".parse().unwrap(),
        )
        .unwrap();
        let injected = BrowserGatewayEndpoint::new(
            BrowserRendezvousId::new([3; contract::ID_LEN]),
            "192.168.4.3:42721".parse().unwrap(),
        )
        .unwrap();
        let catalog = Catalog::new(own);
        catalog.replace_discovered(CatalogSource::LanDiscovery, [lan.clone()]);
        catalog.replace_discovered(CatalogSource::Injected, [injected.clone()]);
        assert_eq!(
            CatalogV1::decode(&catalog.render("127.0.0.1:42721".parse().unwrap()).unwrap())
                .unwrap(),
            vec![
                BrowserGatewayEndpoint::new(own, "127.0.0.1:42721".parse().unwrap()).unwrap(),
                lan,
                injected.clone(),
            ]
        );

        catalog.replace_discovered(CatalogSource::LanDiscovery, []);
        assert_eq!(
            CatalogV1::decode(&catalog.render("127.0.0.1:42721".parse().unwrap()).unwrap())
                .unwrap(),
            vec![
                BrowserGatewayEndpoint::new(own, "127.0.0.1:42721".parse().unwrap()).unwrap(),
                injected,
            ]
        );
    }
}
