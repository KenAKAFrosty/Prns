use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use prns_core::interfaces::shared_instance::rns_rpc::{
    EncodedRpcFrameHeader, RpcFrameHeaderPrefix, AUTHENTICATION_FRAME_MAX_LENGTH,
    RPC_FRAME_MAX_LENGTH,
};

pub(super) async fn write_frame<S: AsyncWrite + Unpin>(
    stream: &mut S,
    payload: &[u8],
) -> std::io::Result<()> {
    if payload.len() > RPC_FRAME_MAX_LENGTH {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    write_frame_header(stream, payload.len()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

pub(super) async fn write_frame_header<S: AsyncWrite + Unpin>(
    stream: &mut S,
    len: usize,
) -> std::io::Result<()> {
    let header = EncodedRpcFrameHeader::new(len)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    stream.write_all(header.as_bytes()).await
}

async fn read_frame_length<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<usize> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let length = match RpcFrameHeaderPrefix::decode(header)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    {
        RpcFrameHeaderPrefix::Complete(length) => length,
        RpcFrameHeaderPrefix::WideLengthFollows => {
            let mut wide = [0u8; 8];
            stream.read_exact(&mut wide).await?;
            RpcFrameHeaderPrefix::decode_wide(wide)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
        }
    };
    Ok(length.as_usize())
}

async fn read_frame_body<S: AsyncRead + Unpin>(
    stream: &mut S,
    len: usize,
) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    body.try_reserve_exact(len)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
    body.resize(len, 0);
    stream.read_exact(&mut body).await?;
    Ok(body)
}

pub(super) async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let len = read_frame_length(stream).await?;
    ensure_frame_length(len, RPC_FRAME_MAX_LENGTH)?;
    read_frame_body(stream, len).await
}

pub(super) async fn read_auth_frame<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> std::io::Result<Vec<u8>> {
    let len = read_frame_length(stream).await?;
    ensure_frame_length(len, AUTHENTICATION_FRAME_MAX_LENGTH)?;
    read_frame_body(stream, len).await
}

pub(super) fn ensure_frame_length(len: usize, maximum: usize) -> std::io::Result<()> {
    if len > maximum {
        Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
    } else {
        Ok(())
    }
}
