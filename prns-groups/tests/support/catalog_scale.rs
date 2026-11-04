//! Explicit host qualification, separate from the ordinary test run. Every row
//! comes from public Create with a real MLS namespace; no catalog seeding.
use super::{directory, execute, options};
use prns_groups::{
    enrollment::{CatalogLimits, Enrollment},
    owned::LocalOwner,
    Client, ConversationPage, ConversationSummary, GroupLifecycle, GroupProfile, IdentityHash,
    Intent, Outcome, PageLimits,
};
use prns_groups_core::GroupId;
use prns_groups_store_redb::RedbLimits;
use std::{
    collections::BTreeSet,
    mem::size_of,
    ops::Bound::{Excluded, Unbounded},
    time::Instant,
};

const TARGET: usize = 1_000;
const PAGE: PageLimits = PageLimits {
    max_rows: 37,
    max_bytes: 4 * 1024,
};
// This is a selected interoperability ceiling, not 256 admitted members. Every
// conversation has one actual local member and its own newly created MLS group.
const PROFILE: GroupProfile = GroupProfile {
    max_members: 256,
    max_record_body_bytes: 2054,
    max_plaintext_bytes: 16_384,
    max_wire_bytes: 65_536,
    max_snapshot_bytes: 262_144,
    max_enrollment_frame_bytes: 524_288,
};

fn catalog_limits() -> CatalogLimits {
    CatalogLimits {
        max_groups: None,
        max_catalog_bytes: 256 * 1024 * 1024,
        max_enrollment_frame_bytes: PROFILE.max_enrollment_frame_bytes,
        max_row_bytes: 512 * 1024,
        max_transaction_rows: 128,
        max_transaction_bytes: 4 * 1024 * 1024,
        max_maintenance_rows: 32,
        max_incoming_invitations: 4,
        max_incoming_per_peer: 2,
        namespace: RedbLimits {
            max_changes: 128,
            max_key_bytes: 256,
            max_value_bytes: 2_097_152,
            max_staged_bytes: 4_194_304,
            max_live_objects: 8192,
            max_live_bytes: 16_777_216,
            max_receipts: 4096,
        },
    }
}

#[derive(Default, serde::Serialize)]
struct QueryWork {
    page_calls: usize,
    returned_rows: usize,
    max_returned_rows: usize,
    max_owned_return_bytes: usize,
    empty_continuations: usize,
    byte_limited_pages: usize,
}
impl QueryWork {
    fn observe(&mut self, page: &ConversationPage, after: Option<GroupId>) {
        // Match the owning query's return accounting: page header, allocated
        // row slots and title capacities. Allocator overhead and decoder
        // scratch are separate and are not inferred from this measurement.
        let owned = size_of::<ConversationPage>()
            + page.items.capacity() * size_of::<ConversationSummary>()
            + page
                .items
                .iter()
                .map(|row| row.title.capacity())
                .sum::<usize>();
        assert!(page.items.len() <= PAGE.max_rows as usize);
        assert!(owned as u64 <= PAGE.max_bytes);
        assert!(page
            .items
            .windows(2)
            .all(|pair| pair[0].group < pair[1].group));
        assert!(page.items.iter().all(|row| {
            after.is_none_or(|after| row.group > after)
                && row.member_count == 1
                && row.ready
                && row.control.is_some()
        }));
        if let Some(next) = page.next {
            assert!(after.is_none_or(|after| next > after));
            assert!(page.items.last().is_none_or(|last| last.group <= next));
        }
        self.page_calls += 1;
        assert!(
            self.page_calls <= TARGET + 14,
            "the fixed-size fixture traversal must finish"
        );
        self.returned_rows += page.items.len();
        self.max_returned_rows = self.max_returned_rows.max(page.items.len());
        self.max_owned_return_bytes = self.max_owned_return_bytes.max(owned);
        self.empty_continuations += usize::from(page.items.is_empty() && page.next.is_some());
        self.byte_limited_pages += usize::from(
            !page.items.is_empty()
                && page.items.len() < PAGE.max_rows as usize
                && page.next.is_some(),
        );
    }
}

async fn drain(
    client: &Client,
    mut after: Option<GroupId>,
    work: &mut QueryWork,
) -> BTreeSet<GroupId> {
    let mut seen = BTreeSet::new();
    loop {
        let page = client.conversations(after, PAGE).await.unwrap();
        work.observe(&page, after);
        for row in page.items {
            assert!(
                seen.insert(row.group),
                "a page must not replay a previous row"
            );
        }
        match page.next {
            Some(next) => after = Some(next),
            None => return seen,
        }
    }
}

async fn create(client: &Client, index: usize) -> GroupId {
    let Outcome::Created(group) = execute(
        client,
        Intent::Create {
            title: format!("Managed scale qualification {index:04}"),
            profile: PROFILE,
        },
    )
    .await
    .unwrap() else {
        panic!("public Create must provision a managed group")
    };
    group
}

async fn selected(client: &Client, group: GroupId, lifecycle: GroupLifecycle) {
    let detail = client.conversation(group, PAGE).await.unwrap();
    assert_eq!(detail.group, group);
    assert_eq!(detail.lifecycle, lifecycle);
    assert_eq!(detail.profile, Some(PROFILE));
    assert_eq!(detail.member_count, 1);
    let members = client.members(group, None, PAGE).await.unwrap();
    assert_eq!(members.control, detail.control);
    assert!(members.next.is_none());
    assert_eq!(members.members.len(), 1);
    assert!(members.members[0].is_local && members.members[0].is_owner);
}

/// Run the already-built isolated test binary with --ignored --exact and a host
/// peak-RSS wrapper. Compiler RSS is not application memory evidence. No network,
/// 256-member admission, mobile, durability-under-power-loss or RF claim is made.
#[tokio::test]
#[ignore = "explicit 1,000 real managed-conversation host qualification"]
async fn thousand_managed_conversations_public_create_reopen_and_live_pages() {
    let directory = directory();
    let path = directory.path().join("managed-scale.redb");
    let identity = IdentityHash::new([81; 16]);
    let limits = catalog_limits();
    assert!(
        limits.max_groups.is_none(),
        "1,000 is a target, not a count quota"
    );
    limits.validate().unwrap();
    PROFILE.validate().unwrap();
    let total = Instant::now();
    let start = Instant::now();
    let owner =
        LocalOwner::create_new(&path, identity, options().with_catalog_limits(limits)).unwrap();
    let startup_ms = start.elapsed().as_millis();
    let client = owner.client();
    let start = Instant::now();
    let mut expected = BTreeSet::new();
    for index in 0..TARGET {
        assert!(expected.insert(create(&client, index).await));
        if (index + 1) % 100 == 0 {
            eprintln!(
                "managed-scale-progress groups={} elapsed_ms={}",
                index + 1,
                start.elapsed().as_millis()
            );
        }
    }
    let create_ms = start.elapsed().as_millis();
    let start = Instant::now();
    let mut initial_work = QueryWork::default();
    assert_eq!(drain(&client, None, &mut initial_work).await, expected);
    assert!(initial_work.byte_limited_pages > 0);
    let initial_query_ms = start.elapsed().as_millis();

    let first = client.conversations(None, PAGE).await.unwrap();
    let cursor = first
        .next
        .expect("the first bounded page is not the whole catalog");
    let seen_group = first.items[0].group;
    let mut reverse = expected.iter().rev().copied();
    let off_page = reverse.next().unwrap();
    let deleted = reverse.next().unwrap();
    let closed = reverse.next().unwrap();
    for group in [off_page, deleted, closed] {
        assert!(group > cursor);
        assert!(!first.items.iter().any(|row| row.group == group));
    }
    let start = Instant::now();
    selected(&client, off_page, GroupLifecycle::Active).await;
    let selected_query_ms = start.elapsed().as_millis();

    // A live cursor survives unrelated committed changes; it is not a global
    // snapshot token. Compare random Create IDs by their actual ordering, never
    // assume that any particular number falls before/after the cursor.
    let start = Instant::now();
    execute(&client, Intent::Close { group: seen_group })
        .await
        .unwrap();
    execute(&client, Intent::Close { group: deleted })
        .await
        .unwrap();
    execute(&client, Intent::DeleteLocal { group: deleted })
        .await
        .unwrap();
    assert!(expected.remove(&deleted));
    execute(&client, Intent::Close { group: closed })
        .await
        .unwrap();
    let mut added_before_cursor = 0;
    for index in TARGET..TARGET + 12 {
        let group = create(&client, index).await;
        added_before_cursor += usize::from(group <= cursor);
        assert!(expected.insert(group));
    }
    let expected_tail: BTreeSet<_> = expected
        .range((Excluded(cursor), Unbounded))
        .copied()
        .collect();
    let mut continued_work = QueryWork::default();
    assert_eq!(
        drain(&client, Some(cursor), &mut continued_work).await,
        expected_tail
    );
    let mut refreshed_work = QueryWork::default();
    assert_eq!(drain(&client, None, &mut refreshed_work).await, expected);
    assert_eq!(expected.len(), TARGET + 11);
    selected(&client, closed, GroupLifecycle::Closed).await;
    let mutation_and_continuation_ms = start.elapsed().as_millis();
    owner.close().await.unwrap();
    drop(client);
    drop(owner);
    let database_bytes = std::fs::metadata(&path).unwrap().len();
    let start = Instant::now();
    {
        let database = redb::Database::open(&path).unwrap();
        assert_eq!(
            Enrollment::open_existing(&database, identity)
                .unwrap()
                .limits()
                .unwrap(),
            limits
        );
    }
    let separate_policy_inspection_ms = start.elapsed().as_millis();

    // Defaults are deliberately different; strict reopen uses persisted policy.
    let start = Instant::now();
    let owner = LocalOwner::open_existing(&path, identity, options()).unwrap();
    let reopen_ms = start.elapsed().as_millis();
    let client = owner.client();
    let mut reopened_work = QueryWork::default();
    let start = Instant::now();
    assert_eq!(drain(&client, None, &mut reopened_work).await, expected);
    selected(&client, off_page, GroupLifecycle::Active).await;
    selected(&client, closed, GroupLifecycle::Closed).await;
    selected(&client, seen_group, GroupLifecycle::Closed).await;
    let reopened_query_ms = start.elapsed().as_millis();
    let beyond_target = create(&client, TARGET + 12).await;
    assert!(expected.insert(beyond_target));
    execute(&client, Intent::DeleteLocal { group: seen_group })
        .await
        .unwrap();
    assert!(expected.remove(&seen_group));
    owner.close().await.unwrap();
    drop(client);
    drop(owner);
    let start = Instant::now();
    let owner = LocalOwner::open_existing(&path, identity, options()).unwrap();
    let final_reopen_ms = start.elapsed().as_millis();
    let client = owner.client();
    let mut final_work = QueryWork::default();
    assert_eq!(drain(&client, None, &mut final_work).await, expected);
    selected(&client, closed, GroupLifecycle::Closed).await;
    selected(&client, beyond_target, GroupLifecycle::Active).await;
    owner.close().await.unwrap();
    drop(client);
    drop(owner);
    eprintln!(
        "managed-scale-result {}",
        serde_json::json!({
            "target_conversations": TARGET, "actual_members_per_group": 1,
            "selected_profile": PROFILE, "catalog_limits": limits,
            "cache_bytes": options().cache_bytes, "created": TARGET + 13,
            "deleted": 2, "remaining": expected.len(),
            "added_before_live_cursor": added_before_cursor,
            "startup_ms": startup_ms, "create_ms": create_ms,
            "initial_query_ms": initial_query_ms, "selected_detail_and_roster_ms": selected_query_ms,
            "mutation_and_continuation_ms": mutation_and_continuation_ms,
            "separate_policy_inspection_ms": separate_policy_inspection_ms,
            "reopen_ms": reopen_ms, "reopened_query_ms": reopened_query_ms,
            "final_reopen_ms": final_reopen_ms, "total_ms": total.elapsed().as_millis(),
            "database_bytes_before_reopen": database_bytes,
            "database_bytes_final": std::fs::metadata(&path).unwrap().len(),
            "page_limits": PAGE, "initial": initial_work, "continued": continued_work,
            "refreshed": refreshed_work, "reopened": reopened_work, "final": final_work,
            "measurement_scope": "host public API calls and owned returned page bytes including header; no internal decoder counter; obtain process peak RSS from isolated test binary wrapper"
        })
    );
}
