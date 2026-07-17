use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::command::SamCommand;
use super::error::SamProtocolError;
use super::reply::{parse_reply, SamReply, SamReplyKind, SamVersion};
use super::MAX_SAM_LINE_BYTES;

pub struct SamControl<Stream> {
    stream: BufReader<Stream>,
}

impl<Stream> SamControl<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn handshake(stream: Stream) -> Result<Self, SamProtocolError> {
        let mut control = Self {
            stream: BufReader::new(stream),
        };
        match control.request(&SamCommand::HelloVersion).await? {
            SamReply::Hello(SamVersion::V3_1) => Ok(control),
            SamReply::Hello(version) => Err(SamProtocolError::InvalidVersion(format!(
                "{}.{}",
                version.major, version.minor
            ))),
            SamReply::Rejected {
                kind,
                rejection,
                message,
            } => Err(SamProtocolError::Rejected {
                kind,
                rejection,
                message,
            }),
            reply => Err(SamProtocolError::UnexpectedReply {
                expected: SamReplyKind::Hello,
                actual: reply.kind(),
            }),
        }
    }

    pub async fn request(&mut self, command: &SamCommand) -> Result<SamReply, SamProtocolError> {
        self.stream
            .get_mut()
            .write_all(command.encode().as_bytes())
            .await?;
        self.stream.get_mut().flush().await?;
        let reply = self.read_reply().await?;
        let expected = command.reply_kind();
        let actual = reply.kind();
        if actual != expected {
            return Err(SamProtocolError::UnexpectedReply { expected, actual });
        }
        Ok(reply)
    }

    pub fn into_stream(self) -> BufReader<Stream> {
        self.stream
    }

    async fn read_reply(&mut self) -> Result<SamReply, SamProtocolError> {
        let mut bytes = Vec::new();
        let read = (&mut self.stream)
            .take(MAX_SAM_LINE_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .await?;
        if read == 0 {
            return Err(SamProtocolError::EndOfStream);
        }
        if bytes.last() != Some(&b'\n') {
            return if read as u64 == MAX_SAM_LINE_BYTES + 1 {
                Err(SamProtocolError::ReplyTooLong)
            } else {
                Err(SamProtocolError::TruncatedReply)
            };
        }
        let line = std::str::from_utf8(&bytes).map_err(|_| SamProtocolError::InvalidUtf8)?;
        parse_reply(line)
    }
}
