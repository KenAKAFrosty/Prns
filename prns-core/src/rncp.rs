pub const APP_NAME: &str = "rncp";
pub const RECEIVE_ASPECT: &str = "receive";
pub const FETCH_PATH: &str = "fetch_file";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchReply {
    Found,
    NotFound,
    NotAllowed,
    RemoteError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RncpCodecError {
    BufferTooShort,
    ValueTooLong,
    Malformed,
    InvalidUtf8,
}

pub fn write_fetch_path(path: &str, output: &mut [u8]) -> Result<usize, RncpCodecError> {
    write_string(path.as_bytes(), output)
}

pub fn parse_fetch_path(input: &[u8]) -> Result<&str, RncpCodecError> {
    let (bytes, consumed) = parse_string(input)?;
    if consumed != input.len() {
        return Err(RncpCodecError::Malformed);
    }
    core::str::from_utf8(bytes).map_err(|_| RncpCodecError::InvalidUtf8)
}

pub fn write_fetch_reply(reply: FetchReply, output: &mut [u8]) -> Result<usize, RncpCodecError> {
    let bytes: &[u8] = match reply {
        FetchReply::Found => &[0xc3],
        FetchReply::NotFound => &[0xc2],
        FetchReply::NotAllowed => &[0xcc, 0xf0],
        FetchReply::RemoteError => &[0xc0],
    };
    if output.len() < bytes.len() {
        return Err(RncpCodecError::BufferTooShort);
    }
    output[..bytes.len()].copy_from_slice(bytes);
    Ok(bytes.len())
}

pub fn parse_fetch_reply(input: &[u8]) -> Result<FetchReply, RncpCodecError> {
    match input {
        [0xc3] => Ok(FetchReply::Found),
        [0xc2] => Ok(FetchReply::NotFound),
        [0xcc, 0xf0] => Ok(FetchReply::NotAllowed),
        [0xc0] => Ok(FetchReply::RemoteError),
        _ => Err(RncpCodecError::Malformed),
    }
}

pub fn write_file_metadata(name: &[u8], output: &mut [u8]) -> Result<usize, RncpCodecError> {
    let mut cursor = 0;
    put(output, &mut cursor, &[0x81, 0xa4, b'n', b'a', b'm', b'e'])?;
    write_binary_at(name, output, &mut cursor)?;
    Ok(cursor)
}

pub fn parse_file_metadata(input: &[u8]) -> Result<&[u8], RncpCodecError> {
    if !input.starts_with(&[0x81, 0xa4, b'n', b'a', b'm', b'e']) {
        return Err(RncpCodecError::Malformed);
    }
    let (name, consumed) = parse_binary(&input[6..])?;
    if consumed + 6 != input.len() {
        return Err(RncpCodecError::Malformed);
    }
    Ok(name)
}

fn write_string(bytes: &[u8], output: &mut [u8]) -> Result<usize, RncpCodecError> {
    let mut cursor = 0;
    match bytes.len() {
        0..=31 => put(output, &mut cursor, &[0xa0 | bytes.len() as u8])?,
        32..=255 => put(output, &mut cursor, &[0xd9, bytes.len() as u8])?,
        256..=65_535 => {
            put(output, &mut cursor, &[0xda])?;
            put(output, &mut cursor, &(bytes.len() as u16).to_be_bytes())?;
        }
        _ => {
            let len = u32::try_from(bytes.len()).map_err(|_| RncpCodecError::ValueTooLong)?;
            put(output, &mut cursor, &[0xdb])?;
            put(output, &mut cursor, &len.to_be_bytes())?;
        }
    }
    put(output, &mut cursor, bytes)?;
    Ok(cursor)
}

fn write_binary_at(
    bytes: &[u8],
    output: &mut [u8],
    cursor: &mut usize,
) -> Result<(), RncpCodecError> {
    match bytes.len() {
        0..=255 => put(output, cursor, &[0xc4, bytes.len() as u8])?,
        256..=65_535 => {
            put(output, cursor, &[0xc5])?;
            put(output, cursor, &(bytes.len() as u16).to_be_bytes())?;
        }
        _ => {
            let len = u32::try_from(bytes.len()).map_err(|_| RncpCodecError::ValueTooLong)?;
            put(output, cursor, &[0xc6])?;
            put(output, cursor, &len.to_be_bytes())?;
        }
    }
    put(output, cursor, bytes)
}

fn parse_string(input: &[u8]) -> Result<(&[u8], usize), RncpCodecError> {
    let marker = *input.first().ok_or(RncpCodecError::Malformed)?;
    let (offset, len): (usize, usize) = match marker {
        0xa0..=0xbf => (1, usize::from(marker & 0x1f)),
        0xd9 => (
            2,
            usize::from(*input.get(1).ok_or(RncpCodecError::Malformed)?),
        ),
        0xda => (3, usize::from(read_u16(input.get(1..3))?)),
        0xdb => {
            let len = usize::try_from(read_u32(input.get(1..5))?)
                .map_err(|_| RncpCodecError::ValueTooLong)?;
            (5, len)
        }
        _ => return Err(RncpCodecError::Malformed),
    };
    let end = offset
        .checked_add(len)
        .ok_or(RncpCodecError::ValueTooLong)?;
    let value = input.get(offset..end).ok_or(RncpCodecError::Malformed)?;
    Ok((value, end))
}

fn parse_binary(input: &[u8]) -> Result<(&[u8], usize), RncpCodecError> {
    let marker = *input.first().ok_or(RncpCodecError::Malformed)?;
    let (offset, len): (usize, usize) = match marker {
        0xc4 => (
            2,
            usize::from(*input.get(1).ok_or(RncpCodecError::Malformed)?),
        ),
        0xc5 => (3, usize::from(read_u16(input.get(1..3))?)),
        0xc6 => {
            let len = usize::try_from(read_u32(input.get(1..5))?)
                .map_err(|_| RncpCodecError::ValueTooLong)?;
            (5, len)
        }
        _ => return Err(RncpCodecError::Malformed),
    };
    let end = offset
        .checked_add(len)
        .ok_or(RncpCodecError::ValueTooLong)?;
    let value = input.get(offset..end).ok_or(RncpCodecError::Malformed)?;
    Ok((value, end))
}

fn put(output: &mut [u8], cursor: &mut usize, bytes: &[u8]) -> Result<(), RncpCodecError> {
    let end = cursor
        .checked_add(bytes.len())
        .ok_or(RncpCodecError::ValueTooLong)?;
    let target = output
        .get_mut(*cursor..end)
        .ok_or(RncpCodecError::BufferTooShort)?;
    target.copy_from_slice(bytes);
    *cursor = end;
    Ok(())
}

fn read_u16(bytes: Option<&[u8]>) -> Result<u16, RncpCodecError> {
    bytes
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(RncpCodecError::Malformed)
}

fn read_u32(bytes: Option<&[u8]>) -> Result<u32, RncpCodecError> {
    bytes
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(RncpCodecError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_values_match_stock_umsgpack() {
        let mut buffer = [0u8; 512];
        let len = write_fetch_path("reports/status.txt", &mut buffer).unwrap();
        assert_eq!(&buffer[..len], b"\xb2reports/status.txt");
        assert_eq!(parse_fetch_path(&buffer[..len]), Ok("reports/status.txt"));

        for (reply, expected) in [
            (FetchReply::Found, &[0xc3][..]),
            (FetchReply::NotFound, &[0xc2][..]),
            (FetchReply::NotAllowed, &[0xcc, 0xf0][..]),
            (FetchReply::RemoteError, &[0xc0][..]),
        ] {
            let len = write_fetch_reply(reply, &mut buffer).unwrap();
            assert_eq!(&buffer[..len], expected);
            assert_eq!(parse_fetch_reply(expected), Ok(reply));
        }
    }

    #[test]
    fn file_metadata_matches_stock_umsgpack() {
        let mut buffer = [0u8; 512];
        let len = write_file_metadata(b"case.bin", &mut buffer).unwrap();
        assert_eq!(
            &buffer[..len],
            &[
                0x81, 0xa4, b'n', b'a', b'm', b'e', 0xc4, 0x08, b'c', b'a', b's', b'e', b'.', b'b',
                b'i', b'n'
            ]
        );
        assert_eq!(parse_file_metadata(&buffer[..len]), Ok(&b"case.bin"[..]));
    }

    #[test]
    fn codecs_reject_trailing_and_truncated_values() {
        assert_eq!(
            parse_fetch_path(b"\xa1a\x00"),
            Err(RncpCodecError::Malformed)
        );
        assert_eq!(
            parse_fetch_path(b"\xd9\x02a"),
            Err(RncpCodecError::Malformed)
        );
        assert_eq!(
            parse_file_metadata(&[0x81, 0xa4, b'n', b'a', b'm', b'e', 0xc4, 2, b'a']),
            Err(RncpCodecError::Malformed)
        );
    }
}
