#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn bounded_projection_round_trip_preserves_sparse_exact_prepared_command() {
    let report = prns_identities_core::DependencyReport::<4>::from_slots(
        &[
            None,
            Some(ServiceDependency::clear(ServiceId(9), 3)),
            None,
            Some(ServiceDependency::clear(ServiceId(4), 7)),
        ],
        true,
        2,
    )
    .unwrap();
    let native: DependencyReport = report.into();
    assert_eq!(native.to_core::<4>().unwrap(), report);
    assert!(native.to_core::<3>().is_err());
    let mut duplicates = native;
    duplicates.entries[0] = duplicates.entries[1];
    assert!(matches!(
        duplicates.to_core::<4>(),
        Err(ApiError::Authority {
            reason: Fault::InvalidServices
        })
    ));
}
#[test]
fn creation_projection_checks_real_capacity_without_truncating_intent() {
    let native = CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("test", None).unwrap(),
        services: vec![ServiceId(2), ServiceId(3)],
    };
    assert!(native.to_core::<1>().is_err());
    assert_eq!(CreateRequest::from(native.to_core::<2>().unwrap()), native);
}
#[test]
fn storage_io_kind_never_implies_an_operation_had_no_effect() {
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::AlreadyExists,
        std::io::ErrorKind::Other,
    ] {
        assert!(matches!(
            ApiError::from(&crate::Error::Io(std::io::Error::new(
                kind,
                "uncertain write"
            ))),
            ApiError::Storage {
                reason: StorageFailure::Io { .. }
            }
        ));
    }
    assert_eq!(
        ApiError::from(&crate::Error::MissingKey),
        ApiError::Storage {
            reason: StorageFailure::MissingKey
        }
    );
}

#[cfg(feature = "contact-card")]
#[test]
fn invalid_card_name_is_rejected_before_queue_admission() {
    use std::{
        future::Future,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll, Waker},
    };
    struct Count(AtomicUsize);
    impl Submit for Count {
        fn submit(&self, _: IdentityJob) -> Result<(), SubmissionFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(SubmissionFailure::Full)
        }
    }
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let client = Client::new(count.clone(), 8, 2)
        .identity(IdentityReference {
            id: LocalIdentityId {
                incarnation: [1; 16],
                sequence: 1,
            },
            public_hash: PublicIdentityHash([2; 16]),
        })
        .unwrap();
    for name in ["line\nbreak".into(), "x".repeat(129), "\u{202e}".into()] {
        let mut request = std::pin::pin!(client.contact_card([3; 10], name));
        assert_eq!(
            request
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(ApiError::InvalidInput))
        );
    }
    assert_eq!(count.0.load(Ordering::SeqCst), 0);
    let mut request = std::pin::pin!(client.contact_card([3; 10], "Telemetry".into()));
    assert_eq!(
        request
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(ApiError::Submission {
            reason: SubmissionFailure::Full
        }))
    );
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
}
