#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use std::{
    future::Future,
    pin::pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

#[derive(Default)]
struct Lane(Mutex<Option<Job>>);
impl Submit for Lane {
    fn submit(&self, job: Job) -> Result<(), ApiError> {
        let mut slot = self.0.lock().unwrap();
        if slot.is_some() {
            return Err(prns_identities_core::SubmissionFailure::Full.into());
        }
        *slot = Some(job);
        Ok(())
    }
}
fn client(lane: Arc<Lane>) -> IdentityAnnounceClient {
    IdentityAnnounceClient::new(
        IdentityReference {
            id: prns_identities_core::LocalIdentityId {
                incarnation: [1; 16],
                sequence: 1,
            },
            public_hash: prns_identities_core::PublicIdentityHash([2; 16]),
        },
        lane,
    )
}
#[test]
fn manual_only_setting_and_queued_result_survive_observer_loss() {
    let lane = Arc::new(Lane::default());
    let client = client(lane.clone());
    let mut context = Context::from_waker(Waker::noop());
    let mut request = Box::pin(client.set_settings(Settings {
        interval_millis: None,
    }));
    assert!(request.as_mut().poll(&mut context).is_pending());
    drop(request);
    let mut saved = None;
    lane.0
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .execute(|identity, request| {
            assert_eq!(identity.id.sequence, 1);
            let Request::SetSettings(settings) = request else {
                panic!("setting")
            };
            saved = Some(settings);
            Ok(Reply::Settings(settings))
        });
    assert_eq!(
        saved,
        Some(Settings {
            interval_millis: None
        })
    );
    let mut request = pin!(client.announce());
    assert!(request.as_mut().poll(&mut context).is_pending());
    lane.0.lock().unwrap().take().unwrap().execute(|_, _| {
        Ok(Reply::Manual(ManualRequest::Throttled {
            retry_after_ms: 42,
        }))
    });
    assert_eq!(
        request.as_mut().poll(&mut context),
        Poll::Ready(Ok(ManualRequest::Throttled { retry_after_ms: 42 }))
    );
}
#[test]
fn invalid_interval_never_enters_owner_lane_and_closed_job_keeps_typed_error() {
    let lane = Arc::new(Lane::default());
    let client = client(lane.clone());
    let mut context = Context::from_waker(Waker::noop());
    let mut invalid = pin!(client.set_settings(Settings {
        interval_millis: Some(1)
    }));
    assert_eq!(
        invalid.as_mut().poll(&mut context),
        Poll::Ready(Err(ApiError::InvalidInput))
    );
    assert!(lane.0.lock().unwrap().is_none());
    let mut read = pin!(client.settings());
    assert!(read.as_mut().poll(&mut context).is_pending());
    lane.0
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .fail(ApiError::Closing);
    assert_eq!(
        read.as_mut().poll(&mut context),
        Poll::Ready(Err(ApiError::Closing))
    );
}
