#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod crc;
mod framing;
mod image;
mod transfer;

pub use crc::{firmware_crc, FirmwareCrc};
pub use framing::{
    Acknowledgement, AcknowledgementDecoder, AcknowledgementError, EncodedFrame, FrameEncodeError,
    ReliableFrameEncoder,
};
pub use image::{
    ApplicationInitPacket, ApplicationVersion, DfuDeviceRevision, DfuDeviceType, DfuImage,
    DfuImageError, SoftdeviceFirmwareId, SoftdeviceRequirements,
};
pub use transfer::{
    DfuBankLayout, DfuTransfer, PendingTransferFrame, RequiredWait, TransferError,
    TransferProgress, TransferState, RELIABLE_FRAME_ATTEMPT_LIMIT,
};
