//! Experimental Ethernet payload: version discriminator, exact frame length, frame.
//! Length framing prevents Ethernet minimum-payload padding entering Reticulum.

const PREFIX: &[u8; 8] = b"PRNSHL\x01\0";
const HEADER_LEN: usize = PREFIX.len() + 2;
pub const DATAGRAM_MTU: usize = 1500;
pub const FRAME_MTU: usize = DATAGRAM_MTU - HEADER_LEN;
const ETHERNET_MIN_PAYLOAD: usize = 46;

#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    WrongProtocol,
    InvalidLength,
    BufferTooSmall,
}

pub fn encode<'a>(frame: &[u8], output: &'a mut [u8]) -> Result<&'a [u8], WireError> {
    if frame.is_empty() || frame.len() > FRAME_MTU {
        return Err(WireError::InvalidLength);
    }
    let output = output
        .get_mut(..HEADER_LEN + frame.len())
        .ok_or(WireError::BufferTooSmall)?;
    output[..PREFIX.len()].copy_from_slice(PREFIX);
    output[PREFIX.len()..HEADER_LEN].copy_from_slice(&(frame.len() as u16).to_be_bytes());
    output[HEADER_LEN..].copy_from_slice(frame);
    Ok(output)
}

pub fn decode(datagram: &[u8]) -> Result<&[u8], WireError> {
    if datagram.get(..PREFIX.len()) != Some(PREFIX) {
        return Err(WireError::WrongProtocol);
    }
    let length = datagram
        .get(PREFIX.len()..HEADER_LEN)
        .ok_or(WireError::InvalidLength)?;
    let length = u16::from_be_bytes([length[0], length[1]]) as usize;
    let end = HEADER_LEN + length;
    if length == 0
        || length > FRAME_MTU
        || (datagram.len() != end
            && !(end < ETHERNET_MIN_PAYLOAD && datagram.len() == ETHERNET_MIN_PAYLOAD))
    {
        return Err(WireError::InvalidLength);
    }
    datagram
        .get(HEADER_LEN..end)
        .ok_or(WireError::InvalidLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_datagrams_recover_the_exact_frame_and_ignore_only_ethernet_padding() {
        let mut output = [0; DATAGRAM_MTU];
        let bytes = encode(b"frame", &mut output).unwrap();
        assert_eq!(decode(bytes), Ok(b"frame".as_slice()));
        assert_eq!(
            decode(&output[..ETHERNET_MIN_PAYLOAD]),
            Ok(b"frame".as_slice())
        );
        assert_eq!(
            decode(&output[..ETHERNET_MIN_PAYLOAD + 1]),
            Err(WireError::InvalidLength)
        );
        output[6] = 2;
        assert_eq!(decode(&output), Err(WireError::WrongProtocol));
        assert_eq!(
            encode(&[1; FRAME_MTU + 1], &mut output),
            Err(WireError::InvalidLength)
        );
        assert_eq!(
            encode(b"frame", &mut [0; 2]),
            Err(WireError::BufferTooSmall)
        );
        let maximum = [5; FRAME_MTU];
        assert_eq!(
            decode(encode(&maximum, &mut output).unwrap()),
            Ok(maximum.as_slice())
        );
    }

    #[test]
    fn every_truncation_and_invalid_length_is_rejected() {
        let mut output = [0; DATAGRAM_MTU];
        let length = encode(&[7; 80], &mut output).unwrap().len();
        for end in 0..length {
            assert!(
                decode(&output[..end]).is_err(),
                "accepted truncation at {end}"
            );
        }
        assert_eq!(decode(&output[..length + 1]), Err(WireError::InvalidLength));
        output[PREFIX.len()..HEADER_LEN].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(decode(&output[..HEADER_LEN]), Err(WireError::InvalidLength));
        output[PREFIX.len()..HEADER_LEN].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(decode(&output), Err(WireError::InvalidLength));
    }
}
