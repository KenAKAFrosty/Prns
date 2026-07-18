use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use prns_core::interfaces::i2p::sam::{parse_reply, SamExchange, SamHello, SamReply};

use super::error::SamControlError;
use super::MAX_SAM_LINE_BYTES;

pub struct SamControl<Stream> {
    stream: BufReader<Stream>,
}

impl<Stream> SamControl<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn handshake(stream: Stream) -> Result<Self, SamControlError> {
        let mut control = Self {
            stream: BufReader::new(stream),
        };
        control.exchange(SamHello).await?;
        Ok(control)
    }

    pub async fn exchange<Exchange>(
        &mut self,
        exchange: Exchange,
    ) -> Result<Exchange::Output, SamControlError>
    where
        Exchange: SamExchange,
    {
        let command = exchange.command();
        self.stream
            .get_mut()
            .write_all(command.encode().as_bytes())
            .await?;
        self.stream.get_mut().flush().await?;
        exchange
            .conclude(self.read_reply().await?)
            .map_err(Into::into)
    }

    pub fn into_stream(self) -> BufReader<Stream> {
        self.stream
    }

    async fn read_reply(&mut self) -> Result<SamReply, SamControlError> {
        let mut bytes = Vec::new();
        let read = (&mut self.stream)
            .take(MAX_SAM_LINE_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .await?;
        if read == 0 {
            return Err(SamControlError::EndOfStream);
        }
        if bytes.last() != Some(&b'\n') {
            return if read as u64 == MAX_SAM_LINE_BYTES + 1 {
                Err(SamControlError::ReplyTooLong)
            } else {
                Err(SamControlError::TruncatedReply)
            };
        }
        let line = std::str::from_utf8(&bytes).map_err(|_| SamControlError::InvalidUtf8)?;
        parse_reply(line).map_err(Into::into)
    }
}
