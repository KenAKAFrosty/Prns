use std::fmt;
use std::io;
use std::net::Ipv4Addr;
use std::str::FromStr;

use tokio::io::BufReader;
use tokio::net::TcpStream;

use super::sam::{
    generate_destination, resolve_destination, I2pAcceptedStream, I2pAddress,
    I2pGeneratedDestination, I2pPrivateDestination, I2pPublicDestination, SamControlError,
    SamProtocolError, SamSession, SamSessionDestination, SamSessionId, SamStreamError,
};
use super::transport::{
    SamBridgeTransport, SamFailureClass, SamSessionTransport, SamTransportError,
};
use crate::tcp::tune_i2p;

#[cfg(test)]
mod tests;

const DEFAULT_SAM_BRIDGE_HOST: &str = "127.0.0.1";
const DEFAULT_SAM_BRIDGE_PORT: u16 = 7656;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamBridgeAddress {
    host: String,
    port: u16,
}

impl SamBridgeAddress {
    pub fn new(value: impl Into<String>) -> Result<Self, SamBridgeAddressError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SamBridgeAddressError::Empty);
        }
        if value.trim() != value {
            return Err(SamBridgeAddressError::SurroundingWhitespace);
        }
        let (host, port) = value
            .rsplit_once(':')
            .ok_or(SamBridgeAddressError::MissingPort)?;
        if host.is_empty() || host.chars().any(char::is_whitespace) || host.contains(':') {
            return Err(SamBridgeAddressError::InvalidHost);
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| SamBridgeAddressError::InvalidPort)?;
        if port == 0 {
            return Err(SamBridgeAddressError::PortZero);
        }
        Ok(Self {
            host: String::from(host),
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn scope(&self) -> SamBridgeScope {
        if self.host.eq_ignore_ascii_case("localhost")
            || self
                .host
                .parse::<Ipv4Addr>()
                .is_ok_and(|address| address.is_loopback())
        {
            SamBridgeScope::Loopback
        } else {
            SamBridgeScope::NonLoopback
        }
    }
}

impl Default for SamBridgeAddress {
    fn default() -> Self {
        Self {
            host: String::from(DEFAULT_SAM_BRIDGE_HOST),
            port: DEFAULT_SAM_BRIDGE_PORT,
        }
    }
}

impl fmt::Display for SamBridgeAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

impl FromStr for SamBridgeAddress {
    type Err = SamBridgeAddressError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamBridgeScope {
    Loopback,
    NonLoopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamBridgeAddressError {
    Empty,
    SurroundingWhitespace,
    MissingPort,
    InvalidHost,
    InvalidPort,
    PortZero,
}

impl fmt::Display for SamBridgeAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("SAM bridge address is empty"),
            Self::SurroundingWhitespace => {
                formatter.write_str("SAM bridge address has surrounding whitespace")
            }
            Self::MissingPort => formatter
                .write_str("SAM bridge address has no port; use host:port, such as 127.0.0.1:7656"),
            Self::InvalidHost => formatter.write_str(
                "SAM bridge host is invalid; RNS 1.3.8 accepts a hostname or IPv4 address",
            ),
            Self::InvalidPort => {
                formatter.write_str("SAM bridge port must be an integer from 1 through 65535")
            }
            Self::PortZero => formatter.write_str("SAM bridge port must not be zero"),
        }
    }
}

impl std::error::Error for SamBridgeAddressError {}

#[derive(Debug)]
pub enum SamBridgeError {
    Connect {
        address: SamBridgeAddress,
        source: io::Error,
    },
    Control(SamControlError),
    Stream(SamStreamError),
}

impl fmt::Display for SamBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { address, source } => {
                write!(
                    formatter,
                    "could not connect to SAM bridge at {address}; verify I2P is running and its SAM interface is enabled: {source}"
                )
            }
            Self::Control(error) => write!(formatter, "{error}"),
            Self::Stream(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SamBridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } => Some(source),
            Self::Control(error) => Some(error),
            Self::Stream(error) => Some(error),
        }
    }
}

impl SamTransportError for SamBridgeError {
    fn failure_class(&self) -> SamFailureClass {
        if self
            .protocol_error()
            .and_then(SamProtocolError::rejection)
            .is_some_and(is_peer_reachability_rejection)
        {
            return SamFailureClass::PeerUnreachable;
        }
        SamFailureClass::SamUnavailable
    }
}

fn is_peer_reachability_rejection(rejection: &super::sam::SamRejection) -> bool {
    matches!(
        rejection,
        super::sam::SamRejection::CantReachPeer
            | super::sam::SamRejection::Timeout
            | super::sam::SamRejection::KeyNotFound
            | super::sam::SamRejection::PeerNotFound
    )
}

impl From<SamControlError> for SamBridgeError {
    fn from(error: SamControlError) -> Self {
        Self::Control(error)
    }
}

impl SamBridgeError {
    fn protocol_error(&self) -> Option<&SamProtocolError> {
        match self {
            Self::Control(error) => error.protocol_error(),
            Self::Stream(error) => error.protocol_error(),
            Self::Connect { .. } => None,
        }
    }
}

impl From<SamStreamError> for SamBridgeError {
    fn from(error: SamStreamError) -> Self {
        Self::Stream(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokioSamBridge {
    address: SamBridgeAddress,
}

impl TokioSamBridge {
    pub fn new(address: SamBridgeAddress) -> Self {
        Self { address }
    }

    pub fn address(&self) -> &SamBridgeAddress {
        &self.address
    }

    pub async fn generate_destination(&self) -> Result<I2pGeneratedDestination, SamBridgeError> {
        Ok(generate_destination(self.open().await?).await?)
    }

    pub async fn resolve_destination(
        &self,
        name: I2pAddress,
    ) -> Result<I2pPublicDestination, SamBridgeError> {
        Ok(resolve_destination(self.open().await?, name).await?)
    }

    pub async fn create_session(
        &self,
        id: SamSessionId,
        destination: SamSessionDestination,
    ) -> Result<TokioSamSession, SamBridgeError> {
        let session = SamSession::create(self.open().await?, id, destination).await?;
        Ok(TokioSamSession {
            bridge: self.clone(),
            session,
        })
    }

    async fn open(&self) -> Result<TcpStream, SamBridgeError> {
        TcpStream::connect((self.address.host(), self.address.port()))
            .await
            .map_err(|source| SamBridgeError::Connect {
                address: self.address.clone(),
                source,
            })
    }
}

impl Default for TokioSamBridge {
    fn default() -> Self {
        Self::new(SamBridgeAddress::default())
    }
}

pub struct TokioSamSession {
    bridge: TokioSamBridge,
    session: SamSession<TcpStream>,
}

impl TokioSamSession {
    pub fn id(&self) -> &SamSessionId {
        self.session.id()
    }

    pub fn private_destination(&self) -> &I2pPrivateDestination {
        self.session.private_destination()
    }

    pub async fn connect(
        &self,
        destination: I2pPublicDestination,
    ) -> Result<BufReader<TcpStream>, SamBridgeError> {
        let stream = self
            .session
            .connect_stream(self.bridge.open().await?, destination)
            .await?;
        tune_i2p(stream.get_ref());
        Ok(stream)
    }

    pub async fn accept(&self) -> Result<I2pAcceptedStream<TcpStream>, SamBridgeError> {
        let accepted = self
            .session
            .accept_stream(self.bridge.open().await?)
            .await?;
        tune_i2p(accepted.stream.get_ref());
        Ok(accepted)
    }
}

impl SamSessionTransport for TokioSamSession {
    type Stream = TcpStream;
    type Error = SamBridgeError;

    fn private_destination(&self) -> &I2pPrivateDestination {
        self.private_destination()
    }

    async fn connect(
        &self,
        destination: I2pPublicDestination,
    ) -> Result<BufReader<Self::Stream>, Self::Error> {
        self.connect(destination).await
    }

    async fn accept(&self) -> Result<I2pAcceptedStream<Self::Stream>, Self::Error> {
        self.accept().await
    }
}

impl SamBridgeTransport for TokioSamBridge {
    type Stream = TcpStream;
    type Session = TokioSamSession;
    type Error = SamBridgeError;

    async fn generate_destination(&self) -> Result<I2pGeneratedDestination, Self::Error> {
        self.generate_destination().await
    }

    async fn resolve_destination(
        &self,
        name: I2pAddress,
    ) -> Result<I2pPublicDestination, Self::Error> {
        self.resolve_destination(name).await
    }

    async fn create_session(
        &self,
        id: SamSessionId,
        destination: SamSessionDestination,
    ) -> Result<Self::Session, Self::Error> {
        self.create_session(id, destination).await
    }
}
