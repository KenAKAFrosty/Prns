use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader};

use super::command::{SamCommand, SamSessionDestination};
use super::control::SamControl;
use super::error::{SamProtocolError, SamStreamError};
use super::reply::{parse_reply, SamReply, SamReplyKind, SamSessionReplyDestination};
use super::value::{I2pAddress, I2pPrivateDestination, I2pPublicDestination, SamSessionId};
use super::MAX_SAM_LINE_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pGeneratedDestination {
    pub public: Option<I2pPublicDestination>,
    pub private: I2pPrivateDestination,
}

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
    ) -> Result<Self, SamProtocolError> {
        let persistent_destination = match &requested_destination {
            SamSessionDestination::Transient => None,
            SamSessionDestination::Persistent(destination) => Some(destination.clone()),
        };
        let mut control = SamControl::handshake(stream).await?;
        let reply = control
            .request(&SamCommand::SessionCreate {
                id: id.clone(),
                destination: requested_destination,
            })
            .await?;
        let returned_destination = match reply {
            SamReply::SessionCreated { destination } => destination,
            SamReply::Rejected {
                kind,
                rejection,
                message,
            } => return Err(rejected(kind, rejection, message)),
            reply => return Err(unexpected(SamReplyKind::Session, reply)),
        };
        let private_destination = match (persistent_destination, returned_destination) {
            (Some(destination), _) => destination,
            (None, SamSessionReplyDestination::Returned(destination)) => destination,
            (None, SamSessionReplyDestination::Omitted) => {
                return Err(SamProtocolError::MissingTransientSessionDestination)
            }
        };
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
    ) -> Result<BufReader<Stream>, SamProtocolError>
    where
        Stream: AsyncRead + AsyncWrite + Unpin,
    {
        let mut control = SamControl::handshake(stream).await?;
        match control
            .request(&SamCommand::StreamConnect {
                id: self.id.clone(),
                destination,
            })
            .await?
        {
            SamReply::StreamReady => Ok(control.into_stream()),
            SamReply::Rejected {
                kind,
                rejection,
                message,
            } => Err(rejected(kind, rejection, message)),
            reply => Err(unexpected(SamReplyKind::Stream, reply)),
        }
    }

    pub async fn accept_stream<Stream>(
        &self,
        stream: Stream,
    ) -> Result<I2pAcceptedStream<Stream>, SamStreamError>
    where
        Stream: AsyncRead + AsyncWrite + Unpin,
    {
        let mut control = SamControl::handshake(stream).await?;
        match control
            .request(&SamCommand::StreamAccept {
                id: self.id.clone(),
            })
            .await?
        {
            SamReply::StreamReady => {}
            SamReply::Rejected {
                kind,
                rejection,
                message,
            } => return Err(rejected(kind, rejection, message).into()),
            reply => return Err(unexpected(SamReplyKind::Stream, reply).into()),
        }
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
) -> Result<I2pGeneratedDestination, SamProtocolError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let mut control = SamControl::handshake(stream).await?;
    match control.request(&SamCommand::DestinationGenerate).await? {
        SamReply::DestinationGenerated { public, private } => {
            Ok(I2pGeneratedDestination { public, private })
        }
        SamReply::Rejected {
            kind,
            rejection,
            message,
        } => Err(rejected(kind, rejection, message)),
        reply => Err(unexpected(SamReplyKind::Destination, reply)),
    }
}

pub async fn resolve_destination<Stream>(
    stream: Stream,
    name: I2pAddress,
) -> Result<I2pPublicDestination, SamProtocolError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let mut control = SamControl::handshake(stream).await?;
    match control.request(&SamCommand::NamingLookup { name }).await? {
        SamReply::NameResolved { destination } => Ok(destination),
        SamReply::Rejected {
            kind,
            rejection,
            message,
        } => Err(rejected(kind, rejection, message)),
        reply => Err(unexpected(SamReplyKind::Naming, reply)),
    }
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
        .map_err(SamProtocolError::from)?;
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
    if line.starts_with("STREAM STATUS ") {
        return match parse_reply(line)? {
            SamReply::Rejected {
                kind,
                rejection,
                message,
            } => Err(rejected(kind, rejection, message).into()),
            reply => Err(unexpected(SamReplyKind::Stream, reply).into()),
        };
    }
    I2pPublicDestination::new(line.trim_end_matches(['\r', '\n']))
        .map_err(SamStreamError::InvalidPeerDestination)
}

fn rejected(
    kind: SamReplyKind,
    rejection: super::reply::SamRejection,
    message: Option<String>,
) -> SamProtocolError {
    SamProtocolError::Rejected {
        kind,
        rejection,
        message,
    }
}

fn unexpected(expected: SamReplyKind, reply: SamReply) -> SamProtocolError {
    SamProtocolError::UnexpectedReply {
        expected,
        actual: reply.kind(),
    }
}
