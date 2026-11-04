#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use futures_util::FutureExt;
use std::{
    future::Future,
    sync::atomic::AtomicUsize,
    task::{Context, Wake},
};

struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct PanicWake;
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        panic!("subscriber waker");
    }
}

#[test]
fn cancellation_preserves_a_hint_and_releases_the_single_waiter() {
    let mut source = Source::default();
    let observer = source.subscribe().unwrap();
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count);
    let mut waiter = Box::pin(observer.next());
    assert!(waiter
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    assert!(matches!(
        observer.next().now_or_never(),
        Some(Err(ApiError::Busy))
    ));
    source.witness().mark_commit_attempt();
    source.publish();
    drop(waiter); // Cancel after notification, before the next poll consumes it.
    assert!(matches!(observer.next().now_or_never(), Some(Ok(()))));
    assert!(observer.next().now_or_never().is_none());
}

#[test]
fn hints_coalesce_and_late_registration_requires_an_initial_snapshot() {
    let mut source = Source::default();
    source.witness().mark_commit_attempt();
    source.publish();
    let observer = source.subscribe().unwrap();
    assert!(observer.next().now_or_never().is_none());
    for _ in 0..10 {
        source.witness().mark_commit_attempt();
        source.publish();
    }
    assert!(matches!(observer.next().now_or_never(), Some(Ok(()))));
    assert!(observer.next().now_or_never().is_none());
    source.publish(); // A read/no-op has no commit witness.
    assert!(observer.next().now_or_never().is_none());
}

#[test]
fn each_panicking_waker_is_isolated_on_publication_and_close() {
    let mut source = Source::default();
    let first = source.subscribe().unwrap();
    let second = source.subscribe().unwrap();
    let panic = Waker::from(Arc::new(PanicWake));
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    let mut first_wait = Box::pin(first.next());
    let mut second_wait = Box::pin(second.next());
    assert!(first_wait
        .as_mut()
        .poll(&mut Context::from_waker(&panic))
        .is_pending());
    assert!(second_wait
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    source.witness().mark_commit_attempt();
    source.publish();
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    assert!(matches!(first_wait.now_or_never(), Some(Ok(()))));
    assert!(matches!(second_wait.now_or_never(), Some(Ok(()))));
    let mut first_wait = Box::pin(first.next());
    let mut second_wait = Box::pin(second.next());
    assert!(first_wait
        .as_mut()
        .poll(&mut Context::from_waker(&panic))
        .is_pending());
    assert!(second_wait
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    drop(source);
    assert_eq!(count.0.load(Ordering::SeqCst), 2);
    assert!(matches!(
        second_wait.now_or_never(),
        Some(Err(ApiError::Unavailable))
    ));
}

#[test]
fn wake_callbacks_run_without_subscriber_locks() {
    struct Reenter(Arc<Mutex<State>>);
    impl Wake for Reenter {
        fn wake(self: Arc<Self>) {
            assert!(self.0.try_lock().is_ok());
        }
    }
    let mut source = Source::default();
    let observer = source.subscribe().unwrap();
    let waker = Waker::from(Arc::new(Reenter(observer.state.clone())));
    let mut next = Box::pin(observer.next());
    assert!(next
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    source.witness().mark_commit_attempt();
    source.publish();
    assert!(matches!(next.now_or_never(), Some(Ok(()))));
}

#[test]
fn registration_is_bounded_and_reclaims_dropped_weak_slots() {
    let mut source = Source::default();
    let mut observers: Vec<_> = (0..MAX_OBSERVATIONS)
        .map(|_| source.subscribe().unwrap())
        .collect();
    assert!(matches!(source.subscribe(), Err(ApiError::Busy)));
    observers.pop();
    observers.push(source.subscribe().unwrap());
    drop(observers);
    let observer = source.subscribe().unwrap();
    assert_eq!(source.subscribers.len(), 1);
    drop(source);
    assert!(matches!(
        observer.next().now_or_never(),
        Some(Err(ApiError::Unavailable))
    ));
}

mod owner {
    use super::*;
    use crate::{
        Attachment, Client, Error, ExecutionContext, GroupManager, Intent, Job, Request, RequestId,
        Submit,
    };
    use prns_groups_core::{ClockInterval, ClockReading, Timestamp};
    use prns_host::IdentityHash;
    use std::collections::VecDeque;
    const NOW: u64 = 1_800_000_000;
    const ID: IdentityHash = IdentityHash::new([7; 16]);
    #[derive(Default)]
    struct Lane(Mutex<VecDeque<Job>>, crate::InputBudget);
    impl Submit for Lane {
        fn input_budget(&self) -> &crate::InputBudget {
            &self.1
        }
        fn submit(&self, job: Job) -> Result<(), Error> {
            self.0.lock().unwrap().push_back(job);
            Ok(())
        }
    }
    impl Lane {
        fn take(&self) -> Job {
            self.0.lock().unwrap().pop_front().unwrap()
        }
    }
    fn context() -> ExecutionContext {
        ExecutionContext {
            clock: ClockReading::Trusted(
                ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
            ),
        }
    }
    fn request(id: u8) -> Request {
        Request {
            id: RequestId::from(crate::RequestToken::new(NOW, [id; 20]).unwrap()),
            intent: Intent::Create {
                profile: Default::default(),
                title: "Observed".into(),
            },
            original_scope: None,
        }
    }
    async fn run<T>(
        future: impl Future<Output = T>,
        manager: &mut GroupManager<'_>,
        lane: &Lane,
    ) -> T {
        let mut future = Box::pin(future);
        assert!(futures_util::poll!(future.as_mut()).is_pending());
        manager.execute(lane.take());
        future.await
    }
    #[tokio::test]
    async fn dropped_reply_publishes_to_sibling_clients_and_unchanged_reads_stay_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let db = redb::Database::create(dir.path().join("store")).unwrap();
        let mut manager = GroupManager::create_managed(&db, ID).unwrap();
        let lane = Arc::new(Lane::default());
        let first = Client::new(lane.clone());
        let second = Client::new(lane.clone());
        let observer = run(first.subscribe(), &mut manager, &lane).await.unwrap();
        drop(second.admit_execute(request(1), context()).unwrap());
        manager.execute(lane.take());
        assert!(matches!(observer.next().now_or_never(), Some(Ok(()))));
        assert_eq!(
            run(
                first.conversations(
                    None,
                    crate::PageLimits {
                        max_rows: 8,
                        max_bytes: 65536
                    }
                ),
                &mut manager,
                &lane
            )
            .await
            .unwrap()
            .items
            .len(),
            1
        );
        assert!(observer.next().now_or_never().is_none());
        assert_eq!(
            run(
                first.conversations(
                    None,
                    crate::PageLimits {
                        max_rows: 8,
                        max_bytes: 65536
                    }
                ),
                &mut manager,
                &lane
            )
            .await
            .unwrap()
            .items
            .len(),
            1
        );
        assert!(observer.next().now_or_never().is_none());
        drop(manager);
        assert!(matches!(
            observer.next().now_or_never(),
            Some(Err(ApiError::Unavailable))
        ));
    }
    fn initialize(db: &redb::Database, incarnation: u8) {
        let tx = db.begin_write().unwrap();
        crate::enrollment::Enrollment::initialize_in(&tx, ID).unwrap();
        prns_groups_store_redb::change::initialize_in(
            &tx,
            prns_groups_store::StoreIncarnation::new([incarnation; 24]).unwrap(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    #[tokio::test]
    async fn attachment_preserves_subscriptions_between_views_and_rejects_retargeting() {
        let dir = tempfile::tempdir().unwrap();
        let db = redb::Database::create(dir.path().join("first")).unwrap();
        initialize(&db, 1);
        let other = redb::Database::create(dir.path().join("second")).unwrap();
        initialize(&other, 2); // Same identity, distinct physical domain incarnation.
        let mut attachment = Attachment::open(&db, ID).unwrap();
        let lane = Arc::new(Lane::default());
        let client = Client::new(lane.clone());
        let observer = {
            let mut view = GroupManager::open_attached(&db, &mut attachment).unwrap();
            run(client.subscribe(), &mut view, &lane).await.unwrap()
        };
        assert!(observer.next().now_or_never().is_none());
        assert!(GroupManager::open_attached(&other, &mut attachment).is_err());
        {
            let mut view = GroupManager::open_attached(&db, &mut attachment).unwrap();
            view.execute_request(request(2), context()).unwrap(); // Direct native path also publishes.
        }
        assert!(matches!(observer.next().now_or_never(), Some(Ok(()))));
        assert!(observer.next().now_or_never().is_none());
        drop(attachment);
        assert!(matches!(
            observer.next().now_or_never(),
            Some(Err(ApiError::Unavailable))
        ));
    }
    #[tokio::test]
    async fn an_error_after_a_real_domain_commit_still_publishes_possible_change() {
        let dir = tempfile::tempdir().unwrap();
        let db = redb::Database::create(dir.path().join("uncertain")).unwrap();
        let tx = db.begin_write().unwrap();
        crate::enrollment::Enrollment::initialize_in(&tx, ID).unwrap();
        crate::retained::initialize_in(
            &tx,
            ID,
            crate::retained::Limits {
                max_operations: 8,
                max_bytes: 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let mut manager = GroupManager::open_managed(&db, ID).unwrap();
        let lane = Arc::new(Lane::default());
        let client = Client::new(lane.clone());
        let observer = run(client.subscribe(), &mut manager, &lane).await.unwrap();
        let request = request(3);
        crate::retained::fault::set(crate::retained::fault::Cut::DomainFinished);
        assert!(matches!(
            manager.submit_retained(
                request.id,
                request.intent,
                context(),
                &mut crate::admission::Unmanaged
            ),
            Err(Error::Unavailable)
        ));
        assert!(matches!(observer.next().now_or_never(), Some(Ok(()))));
        assert_eq!(
            run(
                client.conversations(
                    None,
                    crate::PageLimits {
                        max_rows: 8,
                        max_bytes: 65536
                    }
                ),
                &mut manager,
                &lane
            )
            .await
            .unwrap()
            .items
            .len(),
            1
        );
        assert!(manager.retained_operation(request.id).unwrap().is_some());
    }
}
