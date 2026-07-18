use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader};

use prns_core::interfaces::i2p::sam::{
    parse_incoming_peer_destination, AcceptStream, ConnectStream, CreateSession,
    EstablishedSession, GenerateDestination, I2pAddress, I2pGeneratedDestination,
    I2pPrivateDestination, I2pPublicDestination, ResolveName, SamSessionDestination, SamSessionId,
};

use super::control::SamControl;
use super::error::{SamControlError, SamStreamError};
use super::MAX_SAM_LINE_BYTES;

pub struct SamSession<ControlStream> {
    control: SamControl<ControlStream>,
    id: SamSessionId,
    private_destination: I2pPrivateDestination,
}

impl<ControlStream> SamSession<ControlStream>
where
    ControlStream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn create(
        stream: ControlStream,
        id: SamSessionId,
        requested_destination: SamSessionDestination,
    ) -> Result<Self, SamControlError> {
        let mut control = SamControl::handshake(stream).await?;
        let EstablishedSession {
            id,
            private_destination,
        } = control
            .exchange(CreateSession::new(id, requested_destination))
            .await?;
        Ok(Self {
            control,
            id,
            private_destination,
        })
    }

    pub fn id(&self) -> &SamSessionId {
        &self.id
    }

    pub fn private_destination(&self) -> &I2pPrivateDestination {
        &self.private_destination
    }

    pub fn into_control(self) -> SamControl<ControlStream> {
        self.control
    }

    pub async fn connect_stream<Stream>(
        &self,
        stream: Stream,
        destination: I2pPublicDestination,
    ) -> Result<BufReader<Stream>, SamControlError>
    where
        Stream: AsyncRead + AsyncWrite + Unpin,
    {
        let mut control = SamControl::handshake(stream).await?;
        control
            .exchange(ConnectStream::new(self.id.clone(), destination))
            .await?;
        Ok(control.into_stream())
    }

    pub async fn accept_stream<Stream>(
        &self,
        stream: Stream,
    ) -> Result<I2pAcceptedStream<Stream>, SamStreamError>
    where
        Stream: AsyncRead + AsyncWrite + Unpin,
    {
        let mut control = SamControl::handshake(stream).await?;
        control.exchange(AcceptStream::new(self.id.clone())).await?;
        let mut stream = control.into_stream();
        let peer = read_peer_destination(&mut stream).await?;
        Ok(I2pAcceptedStream { peer, stream })
    }
}

pub struct I2pAcceptedStream<Stream> {
    pub peer: I2pPublicDestination,
    pub stream: BufReader<Stream>,
}

pub async fn generate_destination<Stream>(
    stream: Stream,
) -> Result<I2pGeneratedDestination, SamControlError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let mut control = SamControl::handshake(stream).await?;
    control.exchange(GenerateDestination).await
}

pub async fn resolve_destination<Stream>(
    stream: Stream,
    name: I2pAddress,
) -> Result<I2pPublicDestination, SamControlError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let mut control = SamControl::handshake(stream).await?;
    control.exchange(ResolveName::new(name)).await
}

async fn read_peer_destination<Stream>(
    stream: &mut BufReader<Stream>,
) -> Result<I2pPublicDestination, SamStreamError>
where
    Stream: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let read = (&mut *stream)
        .take(MAX_SAM_LINE_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(SamControlError::from)?;
    if read == 0 {
        return Err(SamStreamError::PeerClosed);
    }
    if bytes.last() != Some(&b'\n') {
        return if read as u64 == MAX_SAM_LINE_BYTES + 1 {
            Err(SamStreamError::PeerDestinationTooLong)
        } else {
            Err(SamStreamError::PeerDestinationTruncated)
        };
    }
    let line =
        std::str::from_utf8(&bytes).map_err(|_| SamStreamError::PeerDestinationInvalidUtf8)?;
    parse_incoming_peer_destination(line).map_err(SamStreamError::PeerIdentification)
}
