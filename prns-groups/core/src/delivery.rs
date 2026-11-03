use core::fmt;

use crate::{Admission, ClockReading, Endpoint, GroupId, PublicationMetadata, RecordId, Seconds};

/// Durable acceptance of the authenticated record and inline body by this exact
/// endpoint incarnation. This does not mean application execution or human read.
/// The caller verifies the receipt's signer and all bindings before applying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupDurableReceipt"))]
pub struct DurableReceipt {
    pub group: GroupId,
    pub record: RecordId,
    pub target: Endpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupReceiptStatus"))]
pub enum ReceiptStatus {
    Awaiting,
    Received,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRemovalStatus"))]
pub enum RemovalStatus {
    NotKnownRemoved,
    Removed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetProgress {
    endpoint: Endpoint,
    receipt: ReceiptStatus,
    removal: RemovalStatus,
}

impl TargetProgress {
    pub const fn pending(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            receipt: ReceiptStatus::Awaiting,
            removal: RemovalStatus::NotKnownRemoved,
        }
    }

    /// Restore facts from the service's authenticated durable record. This does
    /// not verify their origin or authorize a newly received network statement.
    pub const fn restore(
        endpoint: Endpoint,
        receipt: ReceiptStatus,
        removal: RemovalStatus,
    ) -> Self {
        Self {
            endpoint,
            receipt,
            removal,
        }
    }

    pub const fn endpoint(self) -> Endpoint {
        self.endpoint
    }

    pub const fn receipt(self) -> ReceiptStatus {
        self.receipt
    }

    pub const fn removal(self) -> RemovalStatus {
        self.removal
    }
}

/// In-place transitions over caller-owned bounded target storage. A service
/// commits the changed facts transactionally; this core does not persist them.
pub struct DeliveryProgress<'a> {
    metadata: &'a PublicationMetadata,
    targets: &'a mut [TargetProgress],
}

impl<'a> DeliveryProgress<'a> {
    pub fn new(
        metadata: &'a PublicationMetadata,
        targets: &'a mut [TargetProgress],
    ) -> Result<Self, DeliveryError> {
        if targets.is_empty() {
            return Err(DeliveryError::EmptyTargets);
        }
        for (index, target) in targets.iter().enumerate() {
            if targets[..index]
                .iter()
                .any(|previous| previous.endpoint == target.endpoint)
            {
                return Err(DeliveryError::DuplicateTarget(target.endpoint));
            }
        }
        Ok(Self { metadata, targets })
    }

    pub fn targets(&self) -> &[TargetProgress] {
        self.targets
    }

    pub fn apply_authenticated_receipt(
        &mut self,
        receipt: DurableReceipt,
    ) -> Result<ReceiptUpdate, DeliveryError> {
        if receipt.group != self.metadata.group {
            return Err(DeliveryError::WrongGroup);
        }
        if receipt.record != self.metadata.record {
            return Err(DeliveryError::WrongRecord);
        }
        let target = self
            .targets
            .iter_mut()
            .find(|target| target.endpoint == receipt.target)
            .ok_or(DeliveryError::UnknownTarget(receipt.target))?;
        match target.receipt {
            ReceiptStatus::Received => Ok(ReceiptUpdate::AlreadyRecorded),
            ReceiptStatus::Awaiting => {
                target.receipt = ReceiptStatus::Received;
                Ok(ReceiptUpdate::Recorded)
            }
        }
    }

    pub fn apply_authenticated_removal(
        &mut self,
        endpoint: Endpoint,
    ) -> Result<RemovalUpdate, DeliveryError> {
        let target = self
            .targets
            .iter_mut()
            .find(|target| target.endpoint == endpoint)
            .ok_or(DeliveryError::UnknownTarget(endpoint))?;
        match target.removal {
            RemovalStatus::Removed => Ok(RemovalUpdate::AlreadyRecorded),
            RemovalStatus::NotKnownRemoved => {
                target.removal = RemovalStatus::Removed;
                Ok(RemovalUpdate::Recorded)
            }
        }
    }

    pub fn all_received(&self) -> bool {
        self.targets
            .iter()
            .all(|target| target.receipt == ReceiptStatus::Received)
    }

    pub fn summary(&self, clock: ClockReading, allowed_future_skew: Seconds) -> DeliverySummary {
        let admission = self.metadata.validity.admission(clock, allowed_future_skew);
        let mut summary = DeliverySummary {
            original_targets: self.targets.len() as u64,
            received: 0,
            removed: 0,
            pending: 0,
            expired: 0,
            time_uncertain: 0,
        };
        for target in self.targets.iter() {
            if target.removal == RemovalStatus::Removed {
                summary.removed += 1;
            }
            if target.receipt == ReceiptStatus::Received {
                summary.received += 1;
                continue;
            }
            if target.removal == RemovalStatus::Removed {
                continue;
            }
            match admission {
                Admission::Accept => summary.pending += 1,
                Admission::Expired => summary.expired += 1,
                Admission::TimeUncertain => summary.time_uncertain += 1,
            }
        }
        summary
    }
}

/// `removed` counts every known removal, including targets with historical
/// receipt evidence. The final three counts cover only unreceived, nonremoved
/// targets. Expiry and removal never increase `received`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupDeliverySummary"))]
pub struct DeliverySummary {
    pub original_targets: u64,
    pub received: u64,
    pub removed: u64,
    pub pending: u64,
    pub expired: u64,
    pub time_uncertain: u64,
}

impl DeliverySummary {
    pub const fn all_received(self) -> bool {
        self.original_targets != 0 && self.received == self.original_targets
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptUpdate {
    Recorded,
    AlreadyRecorded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalUpdate {
    Recorded,
    AlreadyRecorded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    EmptyTargets,
    DuplicateTarget(Endpoint),
    WrongGroup,
    WrongRecord,
    UnknownTarget(Endpoint),
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid delivery transition: {self:?}")
    }
}

impl core::error::Error for DeliveryError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{
        AuthorizationEra, ClockInterval, ControlId, ControlReference, EndpointId,
        EndpointIncarnation, NamespaceId, PublicationValidity, SchemaId, Timestamp,
    };

    fn endpoint(id: u8, incarnation: u64) -> Endpoint {
        Endpoint {
            id: EndpointId([id; 32]),
            incarnation: EndpointIncarnation::new(incarnation).unwrap(),
        }
    }

    fn metadata() -> PublicationMetadata {
        PublicationMetadata {
            group: GroupId([1; 32]),
            record: RecordId([2; 32]),
            author: endpoint(1, 1),
            namespace: NamespaceId(1),
            schema: SchemaId(1),
            validity: PublicationValidity::new(Timestamp(100), Timestamp(200), Seconds(100))
                .unwrap(),
            original_control: ControlReference {
                head: ControlId([3; 32]),
                era: AuthorizationEra(ControlId([4; 32])),
            },
        }
    }

    fn receipt(metadata: &PublicationMetadata, target: Endpoint) -> DurableReceipt {
        DurableReceipt {
            group: metadata.group,
            record: metadata.record,
            target,
        }
    }

    fn clock(lower: u64, upper: u64) -> ClockReading {
        ClockReading::Trusted(ClockInterval::new(Timestamp(lower), Timestamp(upper)).unwrap())
    }

    #[test]
    fn target_validation_rejects_empty_and_duplicate_sets_without_mutation() {
        let metadata = metadata();
        assert!(matches!(
            DeliveryProgress::new(&metadata, &mut []),
            Err(DeliveryError::EmptyTargets)
        ));
        let target = endpoint(1, 1);
        let mut targets = [TargetProgress::pending(target); 2];
        let before = targets;
        assert!(matches!(
            DeliveryProgress::new(&metadata, &mut targets),
            Err(DeliveryError::DuplicateTarget(value)) if value == target
        ));
        assert_eq!(targets, before);
    }

    #[test]
    fn receipts_require_exact_group_record_and_endpoint_incarnation() {
        let metadata = metadata();
        let target = endpoint(1, 1);
        let mut targets = [TargetProgress::pending(target)];
        let mut progress = DeliveryProgress::new(&metadata, &mut targets).unwrap();
        let valid = receipt(&metadata, target);
        for (receipt, error) in [
            (
                DurableReceipt {
                    group: GroupId([99; 32]),
                    ..valid
                },
                DeliveryError::WrongGroup,
            ),
            (
                DurableReceipt {
                    record: RecordId([99; 32]),
                    ..valid
                },
                DeliveryError::WrongRecord,
            ),
            (
                DurableReceipt {
                    target: endpoint(1, 2),
                    ..valid
                },
                DeliveryError::UnknownTarget(endpoint(1, 2)),
            ),
        ] {
            assert_eq!(progress.apply_authenticated_receipt(receipt), Err(error));
            assert_eq!(progress.targets(), &[TargetProgress::pending(target)]);
        }
        assert_eq!(
            progress.apply_authenticated_receipt(valid),
            Ok(ReceiptUpdate::Recorded)
        );
        assert_eq!(
            progress.apply_authenticated_receipt(valid),
            Ok(ReceiptUpdate::AlreadyRecorded)
        );
        assert!(progress.all_received());
    }

    #[test]
    fn local_author_is_not_implicitly_received_and_all_original_targets_are_required() {
        let metadata = metadata();
        let other = endpoint(2, 1);
        let mut targets = [
            TargetProgress::pending(metadata.author),
            TargetProgress::pending(other),
        ];
        let mut progress = DeliveryProgress::new(&metadata, &mut targets).unwrap();
        assert!(!progress.all_received());
        progress
            .apply_authenticated_receipt(receipt(&metadata, other))
            .unwrap();
        assert!(!progress.all_received());
        assert_eq!(
            progress.summary(clock(100, 100), Seconds(0)),
            DeliverySummary {
                original_targets: 2,
                received: 1,
                removed: 0,
                pending: 1,
                expired: 0,
                time_uncertain: 0,
            }
        );
        progress
            .apply_authenticated_receipt(receipt(&metadata, metadata.author))
            .unwrap();
        assert!(progress.all_received());
    }

    #[test]
    fn late_receipt_adds_evidence_without_erasing_removal_or_extending_deadline() {
        let metadata = metadata();
        let removed = endpoint(1, 1);
        let other = endpoint(2, 1);
        let mut targets = [
            TargetProgress::pending(removed),
            TargetProgress::pending(other),
        ];
        let mut progress = DeliveryProgress::new(&metadata, &mut targets).unwrap();
        assert_eq!(
            progress.apply_authenticated_removal(removed),
            Ok(RemovalUpdate::Recorded)
        );
        assert_eq!(
            progress.apply_authenticated_removal(removed),
            Ok(RemovalUpdate::AlreadyRecorded)
        );
        let uncertain = progress.summary(clock(199, 200), Seconds(0));
        assert_eq!(uncertain.time_uncertain, 1);
        assert_eq!(uncertain.expired, 0);
        assert!(!uncertain.all_received());
        let expired = progress.summary(clock(200, 210), Seconds(0));
        assert_eq!(expired.removed, 1);
        assert_eq!(expired.expired, 1);
        assert_eq!(expired.received, 0);
        assert!(!expired.all_received());
        progress
            .apply_authenticated_receipt(receipt(&metadata, removed))
            .unwrap();
        progress
            .apply_authenticated_receipt(receipt(&metadata, other))
            .unwrap();
        assert_eq!(
            progress.targets()[0],
            TargetProgress::restore(removed, ReceiptStatus::Received, RemovalStatus::Removed)
        );
        assert_eq!(
            progress.summary(clock(210, 210), Seconds(0)),
            DeliverySummary {
                original_targets: 2,
                received: 2,
                removed: 1,
                pending: 0,
                expired: 0,
                time_uncertain: 0,
            }
        );
        assert_eq!(metadata.validity.deadline(), Timestamp(200));
    }
}
