use core::fmt;

use crate::OperationId;

/// A randomly generated namespace for one initialized store. Never reuse an
/// incarnation after erasing/reinitializing that store; zero is not a namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StoreIncarnation([u8; 24]);

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for StoreIncarnation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = <[u8; 24] as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(bytes).map_err(serde::de::Error::custom)
    }
}

impl StoreIncarnation {
    pub const fn new(bytes: [u8; 24]) -> Result<Self, TicketError> {
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0 {
                return Ok(Self(bytes));
            }
            index += 1;
        }
        Err(TicketError::ZeroIncarnation)
    }

    pub const fn as_bytes(&self) -> &[u8; 24] {
        &self.0
    }

    pub const fn first(self) -> OperationTicket {
        OperationTicket {
            incarnation: self,
            sequence: 1,
        }
    }
}

/// Store-scoped monotonic operation identity. Encoding is the 24 incarnation
/// bytes followed by the nonzero sequence as eight big-endian bytes. This is a
/// local durable-storage contract, not a group protocol identifier or MLS epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationTicket {
    incarnation: StoreIncarnation,
    sequence: u64,
}

impl OperationTicket {
    pub const fn new(incarnation: StoreIncarnation, sequence: u64) -> Result<Self, TicketError> {
        if sequence == 0 {
            return Err(TicketError::ZeroSequence);
        }
        Ok(Self {
            incarnation,
            sequence,
        })
    }

    pub const fn incarnation(self) -> StoreIncarnation {
        self.incarnation
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    pub const fn next(self) -> Option<Self> {
        match self.sequence.checked_add(1) {
            Some(sequence) => Some(Self { sequence, ..self }),
            None => None,
        }
    }

    pub const fn operation_id(self) -> OperationId {
        let mut bytes = [0; 32];
        let mut index = 0;
        while index < 24 {
            bytes[index] = self.incarnation.0[index];
            index += 1;
        }
        let sequence = self.sequence.to_be_bytes();
        index = 0;
        while index < sequence.len() {
            bytes[24 + index] = sequence[index];
            index += 1;
        }
        OperationId(bytes)
    }

    pub const fn from_operation_id(operation: OperationId) -> Result<Self, TicketError> {
        let mut namespace = [0; 24];
        let mut index = 0;
        while index < namespace.len() {
            namespace[index] = operation.0[index];
            index += 1;
        }
        let incarnation = match StoreIncarnation::new(namespace) {
            Ok(incarnation) => incarnation,
            Err(error) => return Err(error),
        };
        let mut sequence = [0; 8];
        index = 0;
        while index < sequence.len() {
            sequence[index] = operation.0[24 + index];
            index += 1;
        }
        Self::new(incarnation, u64::from_be_bytes(sequence))
    }
}

impl From<OperationTicket> for OperationId {
    fn from(ticket: OperationTicket) -> Self {
        ticket.operation_id()
    }
}

impl TryFrom<OperationId> for OperationTicket {
    type Error = TicketError;

    fn try_from(operation: OperationId) -> Result<Self, Self::Error> {
        Self::from_operation_id(operation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketError {
    ZeroIncarnation,
    ZeroSequence,
}

impl fmt::Display for TicketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid operation ticket: {self:?}")
    }
}

impl core::error::Error for TicketError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn encoding_is_lossless_at_sequence_boundaries_and_preserves_all_namespace_bytes() {
        let mut bytes = [0; 24];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let incarnation = StoreIncarnation::new(bytes).unwrap();
        for sequence in [1, 255, 256, u32::MAX as u64 + 1, u64::MAX] {
            let ticket = OperationTicket::new(incarnation, sequence).unwrap();
            let encoded = ticket.operation_id();
            assert_eq!(&encoded.0[..24], &bytes);
            assert_eq!(&encoded.0[24..], &sequence.to_be_bytes());
            assert_eq!(OperationTicket::try_from(encoded), Ok(ticket));
        }
        assert_eq!(incarnation.first().sequence(), 1);
        assert_eq!(incarnation.first().next().unwrap().sequence(), 2);
        assert_eq!(
            OperationTicket::new(incarnation, u64::MAX).unwrap().next(),
            None
        );
    }

    #[test]
    fn zero_namespace_and_zero_sequence_cannot_enter_the_ticket_type() {
        assert_eq!(
            StoreIncarnation::new([0; 24]),
            Err(TicketError::ZeroIncarnation)
        );
        assert_eq!(
            OperationTicket::try_from(OperationId([0; 32])),
            Err(TicketError::ZeroIncarnation)
        );
        let incarnation = StoreIncarnation::new([1; 24]).unwrap();
        assert_eq!(
            OperationTicket::new(incarnation, 0),
            Err(TicketError::ZeroSequence)
        );
        let mut encoded = incarnation.first().operation_id();
        encoded.0[24..].fill(0);
        assert_eq!(
            OperationTicket::try_from(encoded),
            Err(TicketError::ZeroSequence)
        );
        assert_ne!(
            incarnation.first(),
            StoreIncarnation::new([2; 24]).unwrap().first()
        );
    }
}
