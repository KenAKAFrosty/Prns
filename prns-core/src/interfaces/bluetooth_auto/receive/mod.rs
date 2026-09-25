#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleReceiveError {
    BufferTooSmall { length: usize, capacity: usize },
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
