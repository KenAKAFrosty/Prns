use std::fmt;
use std::time::Duration;

use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use nusb::DeviceInfo;
use prns_core::identity::{IdentityPublicKeys, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use prns_core::interfaces::usb_auto::{
    BOOTLOADER_ENTRY_CONTROL_INDEX, BOOTLOADER_ENTRY_CONTROL_VALUE,
    CONTROLLER_ENROLL_CONTROL_REQUEST, CONTROLLER_ENROLL_REQUEST_BYTES,
    CONTROLLER_ENROLL_STATUS_BYTES, CONTROLLER_ENROLL_STATUS_REQUEST, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};

const CONTROL_TIMEOUT: Duration = Duration::from_millis(250);
const ENROLMENT_STATUS_POLL: Duration = Duration::from_millis(200);
const ENROLMENT_STATUS_DEADLINE: Duration = Duration::from_secs(10);

const ENROLMENT_TRANSACTION: u32 = 1;
const ENROLMENT_STATUS_SAVED: u8 = 2;
const ENROLMENT_STATUS_FAILED: u8 = 3;

/// Enrols a controller on the attached WebUSB Prns board and returns the board's target
/// public identity, which the board reports in the enrollment status reply.
///
/// Device-level control transfers, so this runs while a daemon holds the bulk interface claim.
pub async fn enrol_webusb_board(
    controller: &IdentityPublicKeys,
) -> Result<IdentityPublicKeys, WebUsbEnrolmentError> {
    let board = single_webusb_board().await?;
    let device = board.open().await.map_err(WebUsbEnrolmentError::open)?;
    let mut payload = [0u8; CONTROLLER_ENROLL_REQUEST_BYTES];
    payload[..4].copy_from_slice(&ENROLMENT_TRANSACTION.to_le_bytes());
    payload[4..].copy_from_slice(&controller.public_key_bytes());
    device
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: CONTROLLER_ENROLL_CONTROL_REQUEST,
                value: BOOTLOADER_ENTRY_CONTROL_VALUE,
                index: BOOTLOADER_ENTRY_CONTROL_INDEX,
                data: &payload,
            },
            CONTROL_TIMEOUT,
        )
        .await
        .map_err(|error| WebUsbEnrolmentError::transfer("enrolment request", &error))?;

    let mut waited = Duration::ZERO;
    while waited < ENROLMENT_STATUS_DEADLINE {
        let reply = device
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: CONTROLLER_ENROLL_STATUS_REQUEST,
                    value: BOOTLOADER_ENTRY_CONTROL_VALUE,
                    index: BOOTLOADER_ENTRY_CONTROL_INDEX,
                    length: CONTROLLER_ENROLL_STATUS_BYTES as u16,
                },
                CONTROL_TIMEOUT,
            )
            .await
            .map_err(|error| WebUsbEnrolmentError::transfer("status request", &error))?;
        if let Some(target) = saved_target(&reply) {
            return Ok(target);
        }
        if reply.first() == Some(&ENROLMENT_STATUS_FAILED) {
            return Err(WebUsbEnrolmentError::BoardRefusedGrant);
        }
        tokio::time::sleep(ENROLMENT_STATUS_POLL).await;
        waited += ENROLMENT_STATUS_POLL;
    }
    Err(WebUsbEnrolmentError::StatusTimeout {
        deadline: ENROLMENT_STATUS_DEADLINE,
    })
}

fn saved_target(reply: &[u8]) -> Option<IdentityPublicKeys> {
    if reply.first() != Some(&ENROLMENT_STATUS_SAVED) {
        return None;
    }
    let transaction = u32::from_le_bytes(reply.get(1..5)?.try_into().ok()?);
    if transaction != ENROLMENT_TRANSACTION {
        return None;
    }
    let target = reply.get(5..)?;
    if target.len() != IDENTITY_PUBLIC_KEY_LEN {
        return None;
    }
    PublicIdentityMaterial::from_slice(target)
        .map(|material| material.public_keys())
        .ok()
}

async fn single_webusb_board() -> Result<DeviceInfo, WebUsbEnrolmentError> {
    let devices = nusb::list_devices()
        .await
        .map_err(WebUsbEnrolmentError::enumeration)?;
    let mut boards = devices.into_iter().filter(|device| {
        device.vendor_id() == WEBUSB_VENDOR_ID && device.product_id() == WEBUSB_PRODUCT_ID
    });
    let board = boards.next().ok_or(WebUsbEnrolmentError::NoBoard)?;
    if boards.next().is_some() {
        return Err(WebUsbEnrolmentError::SeveralBoards);
    }
    Ok(board)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUsbEnrolmentError {
    NoBoard,
    SeveralBoards,
    BoardRefusedGrant,
    StatusTimeout { deadline: Duration },
    Enumeration { reason: String },
    Open { reason: String },
    Transfer { stage: &'static str, reason: String },
}

impl WebUsbEnrolmentError {
    fn enumeration(error: nusb::Error) -> Self {
        Self::Enumeration {
            reason: error.to_string(),
        }
    }

    fn open(error: nusb::Error) -> Self {
        Self::Open {
            reason: error.to_string(),
        }
    }

    fn transfer(stage: &'static str, error: &nusb::transfer::TransferError) -> Self {
        Self::Transfer {
            stage,
            reason: error.to_string(),
        }
    }
}

impl fmt::Display for WebUsbEnrolmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBoard => write!(
                formatter,
                "no Prns WebUSB board (vendor {WEBUSB_VENDOR_ID:#06x}, product {WEBUSB_PRODUCT_ID:#06x}) is attached"
            ),
            Self::SeveralBoards => {
                write!(formatter, "more than one Prns WebUSB board is attached; enrol one board at a time")
            }
            Self::BoardRefusedGrant => write!(formatter, "the board refused the controller grant"),
            Self::StatusTimeout { deadline } => write!(
                formatter,
                "the board did not save the grant within {} seconds",
                deadline.as_secs_f64()
            ),
            Self::Enumeration { reason } => {
                write!(formatter, "USB device list failed: {reason}")
            }
            Self::Open { reason } => write!(formatter, "could not open the board: {reason}"),
            Self::Transfer { stage, reason } => {
                write!(formatter, "USB {stage} failed: {reason}")
            }
        }
    }
}

impl std::error::Error for WebUsbEnrolmentError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u8, target: &[u8]) -> Vec<u8> {
        let mut reply = vec![status];
        reply.extend_from_slice(&ENROLMENT_TRANSACTION.to_le_bytes());
        reply.extend_from_slice(target);
        reply
    }

    #[test]
    fn saved_reply_returns_the_board_target_identity() {
        let target = [7u8; IDENTITY_PUBLIC_KEY_LEN];
        let keys =
            saved_target(&reply(ENROLMENT_STATUS_SAVED, &target)).expect("saved grant reply");
        assert_eq!(keys.public_key_bytes(), target);
    }

    #[test]
    fn pending_and_short_replies_are_not_targets() {
        assert!(saved_target(&reply(1, &[7u8; IDENTITY_PUBLIC_KEY_LEN])).is_none());
        assert!(saved_target(&reply(ENROLMENT_STATUS_SAVED, &[7u8; 31])).is_none());
        assert!(saved_target(&[]).is_none());
    }
}
