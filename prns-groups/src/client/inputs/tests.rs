#![allow(clippy::unwrap_used)]
use super::*;
use crate::{Client, Submit};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicBool, Barrier, Mutex},
};

struct Lane {
    budget: InputBudget,
    jobs: Mutex<VecDeque<Job>>,
    reject: AtomicBool,
}
impl Lane {
    fn new(bytes: usize, count: usize) -> Arc<Self> {
        Arc::new(Self {
            budget: InputBudget::new(ClientLimits {
                max_job_bytes: u64::try_from(bytes).unwrap(),
                max_pending_job_bytes: u64::try_from(bytes * count).unwrap(),
            })
            .unwrap(),
            jobs: Mutex::default(),
            reject: AtomicBool::new(false),
        })
    }
    fn finish(&self) {
        let job = self.jobs.lock().unwrap().pop_front().unwrap();
        job.fail(Error::Unavailable);
    }
    fn used(&self) -> usize {
        self.budget.0.used.load(Ordering::Acquire)
    }
}
impl Submit for Lane {
    fn input_budget(&self) -> &InputBudget {
        &self.budget
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        if self.reject.load(Ordering::Acquire) {
            return Err(Error::Busy);
        }
        self.jobs.lock().unwrap().push_back(job);
        Ok(())
    }
}
fn frame(bytes: Vec<u8>) -> Command {
    Command::EnrollmentHandle {
        requester: crate::IdentityHash::new([1; 16]),
        frame: bytes,
        now: 1_800_000_000,
    }
}

#[test]
fn carrier_allowance_uses_the_same_job_layout_and_checked_allocation_accounting() {
    let payload = vec![0; 4096];
    let required = ClientLimits::required_job_bytes(payload.capacity() as u64).unwrap();
    let limits = ClientLimits {
        max_job_bytes: required,
        max_pending_job_bytes: required,
    };
    let budget = InputBudget::new(limits).unwrap();
    assert_eq!(budget.limits(), limits);
    assert_eq!(input_bytes(&frame(payload)).unwrap() as u64, required);
    assert_eq!(
        ClientLimits::required_job_bytes(u64::MAX),
        Err(ApiError::InvalidInput)
    );
}

#[test]
fn retained_allocation_capacity_not_payload_length_is_charged() {
    let lane = Lane::new(std::mem::size_of::<Job>() + 64, 2);
    let client = Client::new(lane.clone());
    let mut bytes = Vec::with_capacity(1024);
    bytes.push(1);
    assert!(matches!(
        client.enqueue(frame(bytes)),
        Err(Error::InvalidInput)
    ));
    assert_eq!(lane.used(), 0);
    assert!(lane.jobs.lock().unwrap().is_empty());
    let bytes = Vec::with_capacity(32);
    let charged = std::mem::size_of::<Job>() + bytes.capacity();
    let waiter = client.enqueue(frame(bytes)).unwrap();
    assert_eq!(lane.used(), charged);
    drop(waiter);
    assert_eq!(lane.used(), charged);
    lane.finish();
    assert_eq!(lane.used(), 0);
}

#[test]
fn independently_constructed_clients_and_views_share_one_owner_allowance() {
    let bytes = std::mem::size_of::<Job>();
    let lane = Lane::new(bytes, 1);
    let first = Client::new(lane.clone());
    let second = Client::new(lane.clone()).for_group(prns_groups_core::GroupId([2; 32]));
    drop(first.enqueue(Command::Identity).unwrap());
    assert!(matches!(
        second.enqueue(Command::Identity),
        Err(Error::Busy)
    ));
    lane.finish();
    drop(second.enqueue(Command::Identity).unwrap());
    lane.finish();
    assert_eq!(lane.used(), 0);
}

#[test]
fn rejected_admission_and_failed_validation_release_the_reservation() {
    let lane = Lane::new(std::mem::size_of::<Job>() + 128, 1);
    let client = Client::new(lane.clone());
    lane.reject.store(true, Ordering::Release);
    assert!(matches!(
        client.enqueue(Command::Identity),
        Err(Error::Busy)
    ));
    assert_eq!(lane.used(), 0);
    lane.reject.store(false, Ordering::Release);
    let command = Command::SubmitRetained {
        id: crate::RequestId([1; 32]),
        intent: Intent::Create {
            title: " ".into(),
            profile: Default::default(),
        },
        context: crate::ExecutionContext {
            clock: prns_groups_core::ClockReading::Unknown,
        },
    };
    assert!(matches!(client.enqueue(command), Err(Error::InvalidInput)));
    assert_eq!(lane.used(), 0);
}

#[test]
fn concurrent_clients_cannot_overbook_and_cancelled_waiters_do_not_free_jobs() {
    let bytes = std::mem::size_of::<Job>();
    let lane = Lane::new(bytes, 4);
    let barrier = Arc::new(Barrier::new(16));
    let threads = (0..16)
        .map(|_| {
            let lane = lane.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let client = Client::new(lane);
                barrier.wait();
                match client.enqueue(Command::Identity) {
                    Ok(waiter) => {
                        drop(waiter);
                        true
                    }
                    Err(Error::Busy) => false,
                    other => {
                        assert!(other.is_ok());
                        false
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|accepted| *accepted)
            .count(),
        4
    );
    assert_eq!(lane.used(), bytes * 4);
    for _ in 0..4 {
        lane.finish();
    }
    assert_eq!(lane.used(), 0);
}

#[test]
fn byte_policy_does_not_embed_a_second_membership_or_content_profile() {
    let lane = Lane::new(128 * 1024, 1);
    let client = Client::new(lane.clone());
    let targets = (1u64..=256)
        .map(|id| {
            let mut bytes = [0; 32];
            bytes[..8].copy_from_slice(&id.to_be_bytes());
            prns_groups_core::Endpoint {
                id: prns_groups_core::EndpointId(bytes),
                incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
            }
        })
        .collect();
    drop(
        client
            .enqueue(Command::SubmitRetained {
                id: crate::RequestId([1; 32]),
                intent: Intent::Publish(crate::PublicationIntent {
                    group: prns_groups_core::GroupId([2; 32]),
                    namespace: prns_groups_core::NamespaceId(1),
                    schema: prns_groups_core::SchemaId(1),
                    targets: Targets::Explicit(targets),
                    body: vec![3; 32 * 1024],
                }),
                context: crate::ExecutionContext {
                    clock: prns_groups_core::ClockReading::Unknown,
                },
            })
            .unwrap(),
    );
    assert!(lane.used() > 32 * 1024);
    lane.finish();
    assert_eq!(lane.used(), 0);
}
