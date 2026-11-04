#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{groups, local, stores};
use prns_groups::{enrollment::EnrollmentError, ApiError as GroupError, RejectionReason};
use prns_identities::owned;

fn failure(error: network::DriverError) -> Error {
    Error::from(local::Error::Identity(owned::Error::Driver(error)))
}
fn group(error: prns_groups::Error) -> network::DriverError {
    network::DriverError::Local(groups::Error::Schema(groups::SchemaError::Group(error)))
}
#[test]
fn attachment_failures_retain_canonical_group_and_physical_causes() {
    let cases = [
        (
            group(prns_groups::Error::Enrollment(
                EnrollmentError::UnsupportedVersion,
            )),
            AttachmentFailure::Group {
                error: GroupError::Rejected {
                    reason: RejectionReason::UnsupportedVersion,
                },
            },
        ),
        (
            group(prns_groups::Error::Corrupt),
            AttachmentFailure::Group {
                error: GroupError::Corrupt,
            },
        ),
        (
            group(prns_groups::Error::StorageFull),
            AttachmentFailure::Group {
                error: GroupError::StorageFull,
            },
        ),
        (
            network::DriverError::Local(groups::Error::Store(stores::Error::Capacity)),
            AttachmentFailure::Storage {
                reason: StoreFailure::Capacity,
            },
        ),
        (
            network::DriverError::Local(groups::Error::Capacity),
            AttachmentFailure::Capacity,
        ),
    ];
    for (input, expected) in cases {
        let Error::Attachment { reason, detail } = failure(input) else {
            panic!("lost attachment cause")
        };
        assert_eq!(reason, expected);
        assert!(!detail.is_empty());
    }
    let nested = network::DriverError::Local(groups::Error::Store(stores::Error::Physical(
        prns_storage::database::Error::Domain(groups::SchemaError::Group(
            prns_groups::Error::Corrupt,
        )),
    )));
    assert!(matches!(
        failure(nested),
        Error::Attachment {
            reason: AttachmentFailure::Group {
                error: GroupError::Corrupt
            },
            ..
        }
    ));
}
#[test]
fn identity_storage_errors_keep_their_existing_public_classification() {
    let input = local::Error::<network::DriverError>::Identity(owned::Error::Store(
        prns_identities::Error::UnsupportedCatalogFormat,
    ));
    assert_eq!(
        Error::from(input),
        Error::Identity {
            error: ApiError::Storage {
                reason: prns_identities::api::StorageFailure::UnsupportedFormat
            },
        }
    );
}
#[test]
fn attachment_details_are_bounded_without_splitting_utf8() {
    let detail = attachment::detail(&"é".repeat(2048));
    assert!(detail.len() <= 1024);
    assert!(detail.ends_with('é'));
}

#[test]
fn retained_runner_failure_keeps_its_group_cause() {
    let input = network::DriverError::Failure(std::sync::Arc::new(network::Failure::Group(
        prns_groups::Error::Corrupt,
    )));
    assert!(matches!(
        failure(input),
        Error::Attachment {
            reason: AttachmentFailure::Group {
                error: GroupError::Corrupt
            },
            ..
        }
    ));
}

#[test]
fn network_and_local_attachment_use_the_same_retained_conversion() {
    assert_eq!(
        Error::from(group(prns_groups::Error::Corrupt)),
        failure(group(prns_groups::Error::Corrupt))
    );
}
