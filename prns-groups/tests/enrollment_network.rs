//! Public runner APIs over fresh independent stores and real Reticulum Links.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
#[path = "../transport/reticulum/tests/support/wire.rs"]
mod wire;

use prns_groups::{
    run_multi, Client, Clock, Error, ExecutionContext, GroupManager, Intent, Job, Outcome,
    PublicationIntent, Request, RequestId, Submit, SystemClock,
};
use prns_groups_service::{Endpoint, EndpointId, EndpointIncarnation, GroupId};
use prns_host::IdentityHash;
use std::{
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

enum Work {
    Job(Box<Job>),
    Reopen(mpsc::SyncSender<()>),
    Stop,
}
struct Lane(mpsc::SyncSender<Work>, prns_groups::InputBudget);
impl Submit for Lane {
    fn input_budget(&self) -> &prns_groups::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        self.0
            .try_send(Work::Job(Box::new(job)))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => Error::Busy,
                mpsc::TrySendError::Disconnected(_) => Error::Unavailable,
            })
    }
}
struct Worker {
    client: Client,
    sender: mpsc::SyncSender<Work>,
    task: thread::JoinHandle<()>,
}
impl Worker {
    fn open(path: PathBuf, identity: IdentityHash) -> Self {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (ready, readiness) = mpsc::sync_channel(1);
        let client = Client::new(Arc::new(Lane(sender.clone(), Default::default())));
        let task = thread::spawn(move || {
            let mut ready = ready;
            loop {
                let create = !path.exists();
                let database = if create {
                    redb::Database::create(&path)
                } else {
                    redb::Database::open(&path)
                }
                .unwrap();
                let mut manager = if create {
                    GroupManager::create_managed(&database, identity)
                } else {
                    GroupManager::open_managed(&database, identity)
                }
                .unwrap();
                ready.send(()).unwrap();
                loop {
                    match receiver.recv().unwrap() {
                        Work::Job(job) => manager.execute(*job),
                        Work::Reopen(next) => {
                            ready = next;
                            break;
                        }
                        Work::Stop => return,
                    }
                }
                // Close the actual database and manager, retaining only the
                // serialized lane that still owns any subsequently admitted jobs.
            }
        });
        readiness.recv().unwrap();
        Self {
            client,
            sender,
            task,
        }
    }
    fn stop(self) {
        self.sender.send(Work::Stop).unwrap();
        self.task.join().unwrap();
    }
    fn reopen(&self) {
        let (ready, readiness) = mpsc::sync_channel(1);
        self.sender.send(Work::Reopen(ready)).unwrap();
        readiness.recv().unwrap();
    }
}

fn request_token(nonce: u8) -> [u8; 32] {
    static ISSUED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    prns_groups_core::retry::RequestToken::new(*ISSUED.get_or_init(now), [nonce; 20])
        .unwrap()
        .into_bytes()
}

async fn create(client: &Client, request: u8, title: &str) -> GroupId {
    let result = client
        .execute(
            Request {
                id: RequestId(request_token(request)),
                intent: Intent::Create {
                    profile: Default::default(),
                    title: title.into(),
                },
                original_scope: None,
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap();
    let Outcome::Created(group) = result else {
        panic!("expected created group")
    };
    group
}
async fn invite(owner: &Client, member: &Client, group: GroupId, request: u8) {
    let result = owner
        .execute(
            Request {
                id: RequestId(request_token(request)),
                original_scope: Some(owner.group_scope(group).await.unwrap()),
                intent: Intent::Invite {
                    group,
                    recipient: member.identity().await.unwrap(),
                },
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap();
    let Outcome::Invited(invitation) = result else {
        panic!("expected invitation")
    };
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let page = member.invitations(now(), None, QUERY_PAGE).await.unwrap();
            assert!(
                page.next.is_none(),
                "the fixture invitation catalog fits this page"
            );
            if page.items.iter().any(|item| {
                item.id == invitation
                    && item.state == prns_groups::enrollment::InvitationState::Pending
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("invitation reaches the identified recipient without provisioning files");
    member
        .execute(
            Request {
                id: RequestId(request_token(request + 1)),
                original_scope: None,
                intent: Intent::Respond {
                    invitation,
                    accept: true,
                },
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap();
}

async fn members(client: &Client, group: GroupId, count: usize) {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if client
                .for_group(group)
                .network_config()
                .await
                .is_ok_and(|config| config.readers.len() == count)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("member catches up to the current authenticated control");
    let conversation = client.conversation(group, QUERY_PAGE).await.unwrap();
    let roster = client.members(group, None, QUERY_PAGE).await.unwrap();
    assert!(
        roster.next.is_none(),
        "the small fixture roster fits this page"
    );
    let config = client.for_group(group).network_config().await.unwrap();
    assert_eq!(roster.control, config.control);
    assert_eq!(
        conversation.profile,
        Some(prns_groups::GroupProfile::DEFAULT)
    );
    assert_eq!(roster.members.len(), count);
    assert_eq!(conversation.member_count, u64::try_from(count).unwrap());
    assert_eq!(conversation.control.head, config.control.head);
    assert_eq!(
        roster
            .members
            .iter()
            .filter(|member| member.is_local)
            .count(),
        1
    );
    assert_eq!(
        roster
            .members
            .iter()
            .filter(|member| member.is_owner)
            .count(),
        1
    );
    for member in &roster.members {
        assert!(config.readers.contains(&member.endpoint));
        if member.is_local {
            assert_eq!(member.identity, config.local_reticulum_identity);
        } else {
            assert!(config
                .peers
                .iter()
                .any(|peer| peer.endpoint == member.endpoint && peer.identity == member.identity));
        }
        assert_eq!(
            member.is_owner,
            Some(member.identity) == conversation.owner_identity
        );
    }
}

// Accepted enrollment is visible in the catalog before it has an authenticated
// membership. Neither a summary read nor reopening may bypass delivery gating.
async fn pending_membership(client: &Client, group: GroupId) {
    let page = client.conversations(None, QUERY_PAGE).await.unwrap();
    assert!(page.next.is_none());
    let pending = page.items.iter().find(|item| item.group == group).unwrap();
    assert!(!pending.ready);
    assert!(pending.control.is_none());
    assert_eq!(pending.member_count, 0);
    assert!(!pending.capabilities.can_send);
    assert!(matches!(
        client.conversation(group, QUERY_PAGE).await,
        Err(prns_groups::ApiError::Rejected {
            reason: prns_groups::RejectionReason::NotJoined,
        })
    ));
    assert!(matches!(
        client.for_group(group).network_config().await,
        Err(Error::Enrollment(
            prns_groups::enrollment::EnrollmentError::NotJoined
        ))
    ));
}

async fn delivered(client: &Client, group: GroupId, text: &str, receipts: Option<u64>) {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let page = client
                .messages(group, None, 100, SystemClock.now())
                .await
                .unwrap();
            if page.messages.iter().any(|message| {
                message.text == text && receipts.is_none_or(|count| message.received_count == count)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("message and authenticated receipts converge through the network hub");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_managers_create_invite_join_and_sync_multiple_groups_without_fixtures() {
    let directory = tempfile::tempdir().unwrap();
    let mut nodes = wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..3]
        .iter()
        .map(|host| host.client().identity_hash().unwrap())
        .collect();
    let workers: Vec<_> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            Worker::open(
                directory.path().join(format!("member-{index}.redb")),
                *identity,
            )
        })
        .collect();
    let page = workers[0]
        .client
        .conversations(None, QUERY_PAGE)
        .await
        .unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let mut stops = Vec::new();
    let mut tasks = Vec::new();
    for (index, worker) in workers.iter().enumerate() {
        let (sender, receiver) = watch::channel(false);
        stops.push(sender);
        tasks.push(tokio::spawn(run_multi(
            worker.client.clone(),
            nodes.owners[index].client(),
            prns_groups_reticulum::destination_for(identities[index]).unwrap(),
            prns_groups_reticulum::enrollment::destination_for(identities[index]).unwrap(),
            nodes.jobs[index].take().unwrap(),
            nodes.enrollment_jobs[index].take().unwrap(),
            wire::limits(),
            wire::enrollment_limits(),
            receiver,
        )));
    }
    let alice = &workers[0].client;
    let bob = &workers[1].client;
    let carol = &workers[2].client;
    let group = create(alice, 1, "A group created on the network").await;
    invite(alice, bob, group, 2).await;
    tokio::join!(members(alice, group, 2), members(bob, group, 2));
    nodes.state.set_online(1, false);
    alice
        .execute(
            Request {
                id: RequestId(request_token(4)),
                original_scope: Some(alice.group_scope(group).await.unwrap()),
                intent: Intent::Publish(
                    PublicationIntent::text(group, &String::from("before Carol")).unwrap(),
                ),
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap();
    invite(alice, carol, group, 5).await;
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let page = carol.invitations(now(), None, QUERY_PAGE).await.unwrap();
            assert!(
                page.next.is_none(),
                "the fixture invitation catalog fits this page"
            );
            if page
                .items
                .iter()
                .any(|invite| invite.group == group && invite.waiting_for_delivery)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("accepted join waits while owner's current record lacks its receipt");
    assert_eq!(
        alice
            .for_group(group)
            .network_config()
            .await
            .unwrap()
            .readers
            .len(),
        2
    );
    pending_membership(carol, group).await;
    workers[2].reopen();
    let page = carol.invitations(now(), None, QUERY_PAGE).await.unwrap();
    assert!(page.next.is_none());
    assert!(page
        .items
        .iter()
        .any(|invite| invite.group == group && invite.waiting_for_delivery));
    pending_membership(carol, group).await;
    nodes.state.set_online(1, true);
    delivered(bob, group, "before Carol", None).await;
    delivered(alice, group, "before Carol", Some(1)).await;
    tokio::join!(
        members(alice, group, 3),
        members(bob, group, 3),
        members(carol, group, 3)
    );
    bob.execute(
        Request {
            id: RequestId(request_token(7)),
            original_scope: Some(bob.group_scope(group).await.unwrap()),
            intent: Intent::Publish(
                PublicationIntent::text(group, &String::from("after Carol")).unwrap(),
            ),
        },
        ExecutionContext {
            clock: SystemClock.now(),
        },
    )
    .await
    .unwrap();
    tokio::join!(
        delivered(alice, group, "after Carol", None),
        delivered(carol, group, "after Carol", None)
    );
    delivered(bob, group, "after Carol", Some(2)).await;

    let private = create(alice, 8, "Another independent group").await;
    invite(alice, bob, private, 9).await;
    tokio::join!(members(alice, private, 2), members(bob, private, 2));
    alice
        .execute(
            Request {
                id: RequestId(request_token(11)),
                original_scope: Some(alice.group_scope(private).await.unwrap()),
                intent: Intent::Publish(
                    PublicationIntent::text(private, &String::from("separate conversation"))
                        .unwrap(),
                ),
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap();
    delivered(bob, private, "separate conversation", None).await;
    delivered(alice, private, "separate conversation", Some(1)).await;
    let page = bob.conversations(None, QUERY_PAGE).await.unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.next.is_none());
    let page = carol.conversations(None, QUERY_PAGE).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_none());
    assert!(!bob
        .messages(group, None, 100, SystemClock.now())
        .await
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == "separate conversation"));

    // Reopen the sender's MLS/catalog database before another send. Carol misses
    // that transmission; after Bob goes offline she must obtain it from Alice.
    workers[1].reopen();
    nodes.state.set_online(2, false);
    bob.execute(
        Request {
            id: RequestId(request_token(12)),
            original_scope: Some(bob.group_scope(group).await.unwrap()),
            intent: Intent::Publish(
                PublicationIntent::text(group, &String::from("sent after database restart"))
                    .unwrap(),
            ),
        },
        ExecutionContext {
            clock: SystemClock.now(),
        },
    )
    .await
    .unwrap();
    delivered(alice, group, "sent after database restart", None).await;
    delivered(bob, group, "sent after database restart", Some(1)).await;
    assert!(!carol
        .messages(group, None, 100, SystemClock.now())
        .await
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == "sent after database restart"));
    nodes.state.set_online(1, false);
    nodes.state.set_online(2, true);
    delivered(carol, group, "sent after database restart", None).await;
    nodes.state.set_online(1, true);
    delivered(bob, group, "sent after database restart", Some(2)).await;

    for stop in stops {
        stop.send(true).unwrap();
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    for worker in workers {
        worker.stop();
    }
    // Reopen current ratchets/catalogs without any import or enrollment fixture.
    let reopened = Worker::open(directory.path().join("member-1.redb"), identities[1]);
    let page = reopened
        .client
        .conversations(None, QUERY_PAGE)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.next.is_none());
    delivered(&reopened.client, group, "after Carol", Some(2)).await;
    delivered(
        &reopened.client,
        group,
        "sent after database restart",
        Some(2),
    )
    .await;
    delivered(&reopened.client, private, "separate conversation", None).await;
    reopened.stop();
    nodes.stop().await;
}
