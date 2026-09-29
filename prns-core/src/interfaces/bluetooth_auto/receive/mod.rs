use super::BleSource;

/// Preserves the source failure or identifies a reported length outside the supplied storage.
#[derive(Debug, PartialEq, Eq)]
pub enum BleFrameReceiveError<SourceError> {
    Source(SourceError),
    Length(BleReceiveError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleReceiveError {
    BufferTooSmall { length: usize, capacity: usize },
}

/// Receives and validates a whole frame before exposing its bytes. A backend's reported length
/// is untrusted even after a successful read. Failures expose no frame, but the backend may have
/// changed the supplied storage. Cancellation retains the underlying source's semantics.
pub async fn receive_frame<'a, Source: BleSource>(
    source: &mut Source,
    out: &'a mut [u8],
) -> Result<&'a [u8], BleFrameReceiveError<Source::Error>> {
    let length = source
        .recv_frame(out)
        .await
        .map_err(BleFrameReceiveError::Source)?;
    validate_received_frame_length(length, out.len()).map_err(BleFrameReceiveError::Length)?;
    Ok(&out[..length])
}

/// Checks a complete frame or a stream's declared length before copying or waiting for its body.
pub(super) fn validate_received_frame_length(
    length: usize,
    capacity: usize,
) -> Result<(), BleReceiveError> {
    if length > capacity {
        return Err(BleReceiveError::BufferTooSmall { length, capacity });
    }
    Ok(())
}

/// Copies one complete BLE frame. Refusal leaves the entire output unchanged.
pub fn copy_received_frame(frame: &[u8], out: &mut [u8]) -> Result<usize, BleReceiveError> {
    validate_received_frame_length(frame.len(), out.len())?;
    out[..frame.len()].copy_from_slice(frame);
    Ok(frame.len())
}

#[cfg(test)]
mod tests;
