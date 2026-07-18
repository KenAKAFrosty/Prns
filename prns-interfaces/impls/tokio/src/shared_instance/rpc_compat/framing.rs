use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const AUTH_FRAME_MAX_LEN: usize = 256;

pub(super) async fn write_frame<S: AsyncWrite + Unpin>(
    stream: &mut S,
    payload: &[u8],
) -> std::io::Result<()> {
    write_frame_header(stream, payload.len()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

pub(super) async fn write_frame_header<S: AsyncWrite + Unpin>(
    stream: &mut S,
    len: usize,
) -> std::io::Result<()> {
    match i32::try_from(len) {
        Ok(short) => stream.write_all(&short.to_be_bytes()).await,
        Err(_) => {
            let wide = u64::try_from(len)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            stream.write_all(&(-1i32).to_be_bytes()).await?;
            stream.write_all(&wide.to_be_bytes()).await
        }
    }
}

async fn read_frame_length<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<usize> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let signed = i32::from_be_bytes(header);
    let len = if signed == -1 {
        let mut wide = [0u8; 8];
        stream.read_exact(&mut wide).await?;
        usize::try_from(u64::from_be_bytes(wide))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    } else {
        usize::try_from(signed)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    };
    Ok(len)
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
    read_frame_body(stream, len).await
}

pub(super) async fn read_auth_frame<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> std::io::Result<Vec<u8>> {
    let len = read_frame_length(stream).await?;
    if len > AUTH_FRAME_MAX_LEN {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    read_frame_body(stream, len).await
}
