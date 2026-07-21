#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcFrameLength(usize);

pub const RPC_FRAME_MAX_LENGTH: usize = 16_777_216;

impl RpcFrameLength {
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcFrameHeaderPrefix {
    Complete(RpcFrameLength),
    WideLengthFollows,
}

impl RpcFrameHeaderPrefix {
    pub fn decode(bytes: [u8; 4]) -> Result<Self, RpcFrameLengthDecodeError> {
        let signed = i32::from_be_bytes(bytes);
        if signed == -1 {
            return Ok(Self::WideLengthFollows);
        }
        let length =
            usize::try_from(signed).map_err(|_| RpcFrameLengthDecodeError::NegativeShortLength)?;
        Ok(Self::Complete(RpcFrameLength(length)))
    }

    pub fn decode_wide(bytes: [u8; 8]) -> Result<RpcFrameLength, RpcFrameLengthDecodeError> {
        usize::try_from(u64::from_be_bytes(bytes))
            .map(RpcFrameLength)
            .map_err(|_| RpcFrameLengthDecodeError::WideLengthExceedsPlatform)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcFrameLengthDecodeError {
    NegativeShortLength,
    WideLengthExceedsPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedRpcFrameHeader {
    bytes: [u8; 12],
    length: usize,
}

impl EncodedRpcFrameHeader {
    pub fn new(payload_length: usize) -> Result<Self, RpcFrameHeaderEncodeError> {
        let mut bytes = [0u8; 12];
        let length = match i32::try_from(payload_length) {
            Ok(short) => {
                bytes[..4].copy_from_slice(&short.to_be_bytes());
                4
            }
            Err(_) => {
                let wide = u64::try_from(payload_length)
                    .map_err(|_| RpcFrameHeaderEncodeError::LengthExceedsWireFormat)?;
                bytes[..4].copy_from_slice(&(-1i32).to_be_bytes());
                bytes[4..].copy_from_slice(&wide.to_be_bytes());
                12
            }
        };
        Ok(Self { bytes, length })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcFrameHeaderEncodeError {
    LengthExceedsWireFormat,
}
