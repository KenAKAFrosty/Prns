use crate::remote_control::{
    parse_controller_public_keys, RemoteControlControllerAuthority, RemoteControlControllerCard,
    RemoteControlControllerGrant, RemoteControlRequestSet,
};

pub const CONTROLLER_ENROLL_CONTROL_REQUEST: u8 = 0x56;
pub const CONTROLLER_ENROLL_STATUS_REQUEST: u8 = 0x57;
pub const CONTROLLER_ENROLL_REQUEST_BYTES: usize = 68;
pub const CONTROLLER_ENROLL_STATUS_BYTES: usize = 5 + crate::identity::IDENTITY_PUBLIC_KEY_LEN;
/// Serial authorization appends the full expected target identity to the USB request.
pub const SERIAL_CONTROLLER_ENROLL_REQUEST_BYTES: usize =
    CONTROLLER_ENROLL_REQUEST_BYTES + crate::identity::IDENTITY_PUBLIC_KEY_LEN;
/// A serial reply prefixes the existing status receipt with its query nonce.
pub const SERIAL_CONTROLLER_ENROLL_STATUS_BYTES: usize = 4 + CONTROLLER_ENROLL_STATUS_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbControllerEnrollment {
    pub transaction: u32,
    pub grant: RemoteControlControllerGrant,
}

impl UsbControllerEnrollment {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CONTROLLER_ENROLL_REQUEST_BYTES {
            return None;
        }
        let transaction = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
        let identity = parse_controller_public_keys(bytes.get(4..)?)?;
        let grant = RemoteControlControllerGrant::new(
            identity,
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::all(),
        )
        .ok()?;
        Some(Self { transaction, grant })
    }

    /// Decode a serial request only when it pins this exact target and carries
    /// usable public controller keys. Authorization is always Administrator/all.
    pub fn decode_serial(
        bytes: &[u8],
        target_public_key: &[u8; crate::identity::IDENTITY_PUBLIC_KEY_LEN],
    ) -> Option<Self> {
        if bytes.len() != SERIAL_CONTROLLER_ENROLL_REQUEST_BYTES
            || bytes[CONTROLLER_ENROLL_REQUEST_BYTES..] != *target_public_key
        {
            return None;
        }
        let enrollment = Self::decode(&bytes[..CONTROLLER_ENROLL_REQUEST_BYTES])?;
        RemoteControlControllerCard::new(*enrollment.grant.controller()).ok()?;
        Some(enrollment)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbControllerEnrollmentStatus {
    Idle,
    Pending { transaction: u32 },
    Saved { transaction: u32 },
    Failed { transaction: u32 },
}

impl UsbControllerEnrollmentStatus {
    /// Parse the fixed status receipt without claiming that a historical Saved
    /// transaction is still an active grant. The target identity remains public data.
    pub fn decode(bytes: &[u8]) -> Option<(Self, [u8; crate::identity::IDENTITY_PUBLIC_KEY_LEN])> {
        if bytes.len() != CONTROLLER_ENROLL_STATUS_BYTES {
            return None;
        }
        let transaction = u32::from_le_bytes(bytes[1..5].try_into().ok()?);
        let status = match bytes[0] {
            0 if transaction == 0 => Self::Idle,
            1 => Self::Pending { transaction },
            2 => Self::Saved { transaction },
            3 => Self::Failed { transaction },
            _ => return None,
        };
        Some((status, bytes[5..].try_into().ok()?))
    }

    pub fn encode(
        self,
        target_public_key: &[u8; crate::identity::IDENTITY_PUBLIC_KEY_LEN],
    ) -> [u8; CONTROLLER_ENROLL_STATUS_BYTES] {
        let (status, transaction) = match self {
            Self::Idle => (0, 0),
            Self::Pending { transaction } => (1, transaction),
            Self::Saved { transaction } => (2, transaction),
            Self::Failed { transaction } => (3, transaction),
        };
        let mut bytes = [0; CONTROLLER_ENROLL_STATUS_BYTES];
        bytes[0] = status;
        bytes[1..5].copy_from_slice(&transaction.to_le_bytes());
        bytes[5..].copy_from_slice(target_public_key);
        bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct UsbControllerEnrollmentBusy;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{in_memory::InMemoryNodeIdentity, IdentitySigner};

    #[test]
    fn enrollment_requires_complete_public_identity_and_preserves_transaction() {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&[42; 64]);
        let mut wire = [0; CONTROLLER_ENROLL_REQUEST_BYTES];
        wire[..4].copy_from_slice(&123_u32.to_le_bytes());
        wire[4..36].copy_from_slice(identity.encryption_public_key().as_bytes());
        wire[36..].copy_from_slice(identity.signing_public_key().as_bytes());
        let enrollment = UsbControllerEnrollment::decode(&wire).expect("public identity");
        assert_eq!(enrollment.transaction, 123);
        assert_eq!(
            enrollment.grant.authority(),
            RemoteControlControllerAuthority::Administrator
        );
        assert!(UsbControllerEnrollment::decode(&wire[..67]).is_none());
        assert!(
            UsbControllerEnrollment::decode(&[0; CONTROLLER_ENROLL_REQUEST_BYTES + 1]).is_none()
        );
        let receipt = UsbControllerEnrollmentStatus::Saved { transaction: 123 }.encode(&[42; 64]);
        assert_eq!(&receipt[..5], &[2, 123, 0, 0, 0]);
        assert_eq!(&receipt[5..], &[42; 64]);
    }
}
