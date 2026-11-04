//! Ordinary public Client workflows shared by standalone and catalog owners.
//! Hosts, keys, service registration and execution ownership belong to each
//! consumer fixture; these helpers do not provide construction shortcuts.
use prns_groups::{
    create_request_id, Client, Clock, ExecutionContext, GroupLifecycle, IdentityHash, Intent,
    InvitationId, Outcome, PublicationIntent, Request, StoredRecord, SystemClock, Targets,
};
use prns_groups_core::{GroupId, NamespaceId, RecordId, SchemaId};
use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
pub async fn eventually<F, R>(description: &str, mut condition: F)
where
    F: FnMut() -> R,
    R: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(60), async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect(description);
}
pub async fn execute(client: &Client, intent: Intent) -> Outcome {
    let description = format!("{intent:?}");
    let original_scope = match intent.group() {
        Some(group) => Some(client.group_scope(group).await.unwrap()),
        None => None,
    };
    client
        .execute(
            Request {
                id: create_request_id().unwrap(),
                intent,
                original_scope,
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{description}: {error:?}"))
}
pub async fn create(client: &Client, title: &str) -> GroupId {
    let Outcome::Created(group) = execute(
        client,
        Intent::Create {
            profile: Default::default(),
            title: title.into(),
        },
    )
    .await
    else {
        panic!("created group");
    };
    group
}
pub async fn invite(client: &Client, group: GroupId, recipient: IdentityHash) -> InvitationId {
    let Outcome::Invited(invitation) = execute(client, Intent::Invite { group, recipient }).await
    else {
        panic!("invitation");
    };
    invitation
}
pub async fn members(client: &Client, group: GroupId) {
    eventually(
        "both consenting identities share the authenticated control",
        || async {
            client
                .for_group(group)
                .network_config()
                .await
                .is_ok_and(|config| config.readers.len() == 2)
        },
    )
    .await;
}
pub async fn lifecycle(client: &Client, group: GroupId, expected: GroupLifecycle) {
    eventually("authenticated lifecycle notice arrives", || async {
        client
            .conversation(group, QUERY_PAGE)
            .await
            .unwrap()
            .lifecycle
            == expected
    })
    .await;
}
pub async fn record(client: &Client, group: GroupId, id: RecordId, receipt: bool) -> StoredRecord {
    eventually(
        "record and authenticated device receipt converge",
        || async {
            client
                .records(group, None, 100, SystemClock.now())
                .await
                .unwrap()
                .records
                .iter()
                .any(|item| {
                    item.record.metadata.record == id
                        && (!receipt || item.progress.summary.all_received())
                })
        },
    )
    .await;
    client
        .records(group, None, 100, SystemClock.now())
        .await
        .unwrap()
        .records
        .into_iter()
        .find(|item| item.record.metadata.record == id)
        .unwrap()
}
pub async fn publish(client: &Client, publication: PublicationIntent) -> RecordId {
    let Outcome::Published(record) = execute(client, Intent::Publish(publication)).await else {
        panic!("published record");
    };
    record
}
pub fn binary(group: GroupId, size: usize) -> PublicationIntent {
    PublicationIntent {
        group,
        namespace: NamespaceId(0x5345_4e53),
        schema: SchemaId(1),
        targets: Targets::OtherMembers,
        body: (0..size).map(|i| ((i * 73 + i / 7) % 256) as u8).collect(),
    }
}
