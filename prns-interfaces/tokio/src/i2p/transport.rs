use std::error::Error;

use tokio::io::{AsyncRead, AsyncWrite, BufReader};

use super::sam::{
    I2pAcceptedStream, I2pAddress, I2pGeneratedDestination, I2pPrivateDestination,
    I2pPublicDestination, SamSessionDestination, SamSessionId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamFailureClass {
    SamUnavailable,
    PeerUnreachable,
}

pub trait SamTransportError: Error + Send + Sync + 'static {
    fn failure_class(&self) -> SamFailureClass;
}

#[allow(async_fn_in_trait)]
pub trait SamSessionTransport: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Error: SamTransportError;

    fn private_destination(&self) -> &I2pPrivateDestination;

    async fn connect(
        &self,
        destination: I2pPublicDestination,
    ) -> Result<BufReader<Self::Stream>, Self::Error>;

    async fn accept(&self) -> Result<I2pAcceptedStream<Self::Stream>, Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait SamBridgeTransport: Clone + Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Session: SamSessionTransport<Stream = Self::Stream, Error = Self::Error>;
    type Error: SamTransportError;

    async fn generate_destination(&self) -> Result<I2pGeneratedDestination, Self::Error>;

    async fn resolve_destination(
        &self,
        name: I2pAddress,
    ) -> Result<I2pPublicDestination, Self::Error>;

    async fn create_session(
        &self,
        id: SamSessionId,
        destination: SamSessionDestination,
    ) -> Result<Self::Session, Self::Error>;
}
