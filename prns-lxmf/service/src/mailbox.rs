//! Durable direct-message records and transactions for an application-owned `redb`.
//!
//! This module never opens or owns a database. The application that already owns
//! its database submits [`MailboxRequest`] values to its one blocking owner and
//! calls [`execute_mailbox_request`] there.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::ops::Bound;
use std::pin::Pin;
use std::string::String;
use std::vec::Vec;

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{DirectSendFailure, LxmfDirection, LxmfVerification};
use prns_lxmf_wire::{MessageView, WireLimits, MAX_BASIC_LXMF_WIRE_BYTES, WIRE_HEADER_LENGTH};

/// Domain-owned mailbox policy, counters and format; independent of other schemas.
pub const LXMF_META: TableDefinition<&str, u64> = TableDefinition::new("prns_lxmf_metadata_v2");
/// Durable LXMF records keyed by a local monotonically increasing identifier.
pub const LXMF_MESSAGES: TableDefinition<u64, &[u8]> = TableDefinition::new("lxmf_messages");
/// Inbound LXMF message-ID deduplication index.
pub const LXMF_INBOUND_IDS: TableDefinition<&[u8], u64> = TableDefinition::new("lxmf_inbound_ids");

/// Metadata key holding the next unallocated local record identifier.
pub const NEXT_LXMF_RECORD_ID_KEY: &str = "next_lxmf_record_id";
/// Metadata key advanced in the same transaction as every message mutation.
pub const LXMF_REVISION_KEY: &str = "lxmf_revision";
/// Initial value for [`NEXT_LXMF_RECORD_ID_KEY`].
pub const INITIAL_NEXT_LXMF_RECORD_ID: u64 = 1;
/// Initial durable mailbox revision.
pub const INITIAL_LXMF_REVISION: u64 = 0;

/// Metadata-only projection for local activity consumers. Positions are never
/// reused; receipt updates and duplicate ingress do not create new activity.
#[derive(Debug, Clone, Copy)]
pub struct MailboxActivityRecord {
    pub position: u64,
    pub peer: [u8; 16],
    pub incoming: bool,
}

/// Scan at most `limit` newest retained records, without mutating the mailbox.
/// A true second result means older records were deliberately omitted.
pub fn read_mailbox_activity(
    database: &Database,
    limit: usize,
) -> Result<(Vec<MailboxActivityRecord>, bool), MailboxFailure> {
    if limit == 0 || limit > 512 {
        return Err(MailboxFailure::unavailable("activity scan bound"));
    }
    let read = database
        .begin_read()
        .map_err(|e| classify_redb_error(e.into(), "read activity"))?;
    let table = read
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "read activity records"))?;
    let _header = policy::read(&read)?;
    let mut records = Vec::with_capacity(limit);
    let mut partial = false;
    for entry in table
        .range(1..)
        .map_err(|e| classify_redb_error(e.into(), "scan activity"))?
        .rev()
    {
        if records.len() == limit {
            partial = true;
            break;
        }
        let (key, value) =
            entry.map_err(|e| classify_redb_error(e.into(), "read activity record"))?;
        let record = decode_record(key.value(), value.value())?;
        let incoming = matches!(record.record_kind, StoredRecordKind::Inbound { .. });
        records.push(MailboxActivityRecord {
            position: key.value(),
            peer: if incoming {
                record.source
            } else {
                record.destination
            },
            incoming,
        });
    }
    Ok((records, partial))
}

/// Check a displayed position belongs to the captured peer. The caller owns
/// persistence of its local seen cursor; this does not send a read receipt.
pub fn mailbox_activity_position(
    database: &Database,
    peer: [u8; 16],
    position: u64,
) -> Result<bool, MailboxFailure> {
    if position == 0 {
        return Ok(false);
    }
    let read = database
        .begin_read()
        .map_err(|e| classify_redb_error(e.into(), "read activity position"))?;
    let table = read
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "open activity position"))?;
    let Some(value) = table
        .get(position)
        .map_err(|e| classify_redb_error(e.into(), "find activity position"))?
    else {
        return Ok(false);
    };
    let record = decode_record(position, value.value())?;
    let actual = if matches!(record.record_kind, StoredRecordKind::Inbound { .. }) {
        record.source
    } else {
        record.destination
    };
    Ok(actual == peer)
}

/// A boxed request future implemented by the application's one database owner.
pub type MailboxFuture<'a> =
    Pin<Box<dyn Future<Output = Result<MailboxReply, MailboxFailure>> + Send + 'a>>;

/// Narrow command port into the application's existing database owner.
///
/// Implementations must not open another database. A native aggregate typically
/// implements this by placing the request on the same bounded lane used by its
/// contact operations. Once the returned future admits a mutation to that owner,
/// dropping the future is not cancellation: the caller or its lifecycle owner
/// must retain it through the definitive commit or failure reply.
pub trait MailboxSubmitter: Send + Sync + 'static {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_>;
}

/// Failure classification preserved through the aggregate boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxFailure {
    Busy,
    /// A selected count/byte/query policy refused admission; no row was removed.
    Capacity,
    Unavailable(String),
    ResetRequired(String),
}

impl MailboxFailure {
    fn unavailable(detail: impl Into<String>) -> Self {
        Self::Unavailable(detail.into())
    }

    fn reset_required(reason: impl Into<String>) -> Self {
        Self::ResetRequired(reason.into())
    }
}

/// Public durable/query projection. The attempt generation deliberately remains private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableLxmfMessage {
    pub local_record_id: u64,
    pub message_id: [u8; 32],
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub timestamp_unix_ms: u64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    pub exact_wire: Vec<u8>,
    pub direction: LxmfDirection,
    pub verification: LxmfVerification,
    pub delivery_state: DurableLxmfDeliveryState,
    generation: Option<u64>,
}

impl DurableLxmfMessage {
    #[cfg(any(test, feature = "tokio-runtime"))]
    pub(crate) const fn generation(&self) -> Option<u64> {
        self.generation
    }

    #[cfg(feature = "tokio-runtime")]
    pub(crate) fn project_sending(&mut self, generation: u64) -> bool {
        if self.generation == Some(generation) {
            if let DurableLxmfDeliveryState::Queued { failed_attempts } = self.delivery_state {
                self.delivery_state = DurableLxmfDeliveryState::Sending { failed_attempts };
                return true;
            }
        }
        false
    }
}

/// Durable state plus the process-local `Sending` projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableLxmfDeliveryState {
    Received,
    Queued {
        failed_attempts: u64,
    },
    Sending {
        failed_attempts: u64,
    },
    Delivered {
        delivered_at_millis: u64,
        rtt_millis: Option<u64>,
    },
    Failed {
        failed_attempts: u64,
        last_failure: DirectSendFailure,
    },
    Cancelled {
        cancelled_at_millis: u64,
    },
}

/// One already-composed outbound wire accepted for durable insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutboundMessage {
    pub message_id: [u8; 32],
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub timestamp_unix_ms: u64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    pub exact_wire: Vec<u8>,
}

/// One parsed and classified proof-first inbound wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInboundMessage {
    pub message_id: [u8; 32],
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub timestamp_unix_ms: u64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    pub exact_wire: Vec<u8>,
    pub verification: LxmfVerification,
}

/// Active attempt identity used by every conditional terminal transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AttemptKey {
    pub local_record_id: u64,
    pub generation: u64,
}

/// Terminal result of one path/Link/proof attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptCompletion {
    Delivered {
        delivered_at_millis: u64,
        rtt_millis: Option<u64>,
    },
    Failed {
        failure: DirectSendFailure,
    },
}

/// Direction filter for mailbox queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxDirectionFilter {
    Inbox,
    Outbox,
}

/// Reverse-scan query. `before` is an exclusive local record ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxListRequest {
    pub peer: Option<[u8; 16]>,
    pub direction: Option<MailboxDirectionFilter>,
    pub before: Option<u64>,
    pub limit: usize,
}

/// Latest record per peer, ordered by descending local record ID.
///
/// `before` is exclusive and refers to a conversation's latest record, not its
/// timestamp. The scan can visit the whole mailbox, but only materializes the
/// requested page and retains peer IDs for deduplication; it copies no full
/// mailbox. A newer message can move a conversation ahead of an existing cursor,
/// so consumers should refresh the first page after mailbox mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxConversationListRequest {
    pub before: Option<u64>,
    pub limit: usize,
}

/// Closed mailbox request language executed on the application's owner thread.
#[derive(Debug)]
pub enum MailboxRequest {
    Policy,
    InsertOutbound(NewOutboundMessage),
    InsertInbound(NewInboundMessage),
    CompleteAttempt {
        key: AttemptKey,
        completion: AttemptCompletion,
    },
    Retry {
        local_record_id: u64,
    },
    Cancel {
        local_record_id: u64,
        cancelled_at_millis: u64,
    },
    List(MailboxListRequest),
    ListConversations(MailboxConversationListRequest),
    Replay(MailboxReplayRequest),
}

/// One request's typed result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxReply {
    Policy(MailboxLimits),
    QueuedPage {
        messages: Vec<DurableLxmfMessage>,
        next: Option<u64>,
        revision: u64,
    },
    OutboundInserted {
        message: DurableLxmfMessage,
        revision: u64,
    },
    Inbound(InboundInsertOutcome),
    Completion(CompletionTransition),
    Retry(RetryTransition),
    Cancel(CancelTransition),
    Listed {
        messages: Vec<DurableLxmfMessage>,
        revision: u64,
    },
    ConversationsListed {
        messages: Vec<DurableLxmfMessage>,
        revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundInsertOutcome {
    Inserted {
        message: DurableLxmfMessage,
        revision: u64,
    },
    VerificationUpgraded {
        message: DurableLxmfMessage,
        revision: u64,
    },
    Duplicate {
        local_record_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionTransition {
    Applied {
        message: DurableLxmfMessage,
        revision: u64,
    },
    Stale {
        current: Option<DurableLxmfDeliveryState>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryTransition {
    Accepted {
        message: DurableLxmfMessage,
        revision: u64,
    },
    NotFound,
    NotFailed {
        current: DurableLxmfDeliveryState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelTransition {
    Cancelled {
        message: DurableLxmfMessage,
        revision: u64,
    },
    NotFound,
    AlreadyDelivered,
    AlreadyCancelled,
    NotCancellable {
        current: DurableLxmfDeliveryState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRecord {
    message_id: [u8; 32],
    source: [u8; 16],
    destination: [u8; 16],
    timestamp_unix_ms: u64,
    #[serde(deserialize_with = "policy::bytes")]
    title: Vec<u8>,
    #[serde(deserialize_with = "policy::bytes")]
    content: Vec<u8>,
    #[serde(deserialize_with = "policy::bytes")]
    exact_wire: Vec<u8>,
    record_kind: StoredRecordKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum StoredRecordKind {
    Inbound {
        verification: StoredVerification,
        // Existing records have one mutation. Preserve that interpretation on
        // reopen while accounting for the sole permitted verification upgrade.
        #[serde(default, skip_serializing_if = "is_false")]
        verification_upgraded: bool,
    },
    Outbound {
        generation: u64,
        failed_attempts: u64,
        state: StoredOutboundState,
    },
}

const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredVerification {
    Verified,
    SourceUnknown,
    InvalidSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum StoredOutboundState {
    Queued,
    Delivered {
        delivered_at_millis: u64,
        rtt_millis: RequiredNullableU64,
    },
    Failed {
        last_failure: StoredFailure,
    },
    Cancelled {
        cancelled_at_millis: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredFailure {
    NoRoute,
    LinkFailed,
    DeliveryTimedOut,
    LocalNodeStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct RequiredNullableU64(Option<u64>);

impl<'de> Deserialize<'de> for RequiredNullableU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NullableU64Visitor;

        impl<'de> Visitor<'de> for NullableU64Visitor {
            type Value = RequiredNullableU64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an unsigned 64-bit integer or null")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableU64(Some(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableU64(None))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableU64(None))
            }
        }

        deserializer.deserialize_any(NullableU64Visitor)
    }
}

/// Add mailbox tables and counters to an application database's first-create transaction.
pub fn initialize_mailbox_tables(write: &redb::WriteTransaction) -> Result<(), MailboxFailure> {
    initialize_mailbox_tables_with_limits(write, MailboxLimits::default())
}

/// First-create only. Reopening uses the exact persisted policy and never
/// changes limits, creates missing tables or resets an unsupported format.
pub fn initialize_mailbox_tables_with_limits(
    write: &redb::WriteTransaction,
    limits: MailboxLimits,
) -> Result<(), MailboxFailure> {
    policy::initialize(write, limits)
}

/// Validate all mailbox tables, counters, records, and index relationships.
pub fn validate_mailbox_database(database: &Database) -> Result<(), MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox"))?;
    policy::inspect(&read)
}

/// Execute one complete mailbox operation on the calling database owner thread.
pub fn execute_mailbox_request(
    database: &Database,
    request: MailboxRequest,
) -> Result<MailboxReply, MailboxFailure> {
    match request {
        MailboxRequest::Policy => {
            let read = database
                .begin_read()
                .map_err(|e| classify_redb_error(e.into(), "read mailbox policy"))?;
            policy::inspect(&read)?;
            Ok(MailboxReply::Policy(policy::read(&read)?.limits))
        }
        MailboxRequest::InsertOutbound(message) => insert_outbound(database, message),
        MailboxRequest::InsertInbound(message) => insert_inbound(database, message),
        MailboxRequest::CompleteAttempt { key, completion } => {
            complete_attempt(database, key, completion)
        }
        MailboxRequest::Retry { local_record_id } => retry(database, local_record_id),
        MailboxRequest::Cancel {
            local_record_id,
            cancelled_at_millis,
        } => cancel(database, local_record_id, cancelled_at_millis),
        MailboxRequest::List(request) => list(database, request),
        MailboxRequest::ListConversations(request) => list_conversations(database, request),
        MailboxRequest::Replay(request) => replay::page(database, request),
    }
}

fn insert_outbound(
    database: &Database,
    message: NewOutboundMessage,
) -> Result<MailboxReply, MailboxFailure> {
    let write = begin_write(database, "insert outbound LXMF message")?;
    let local_record_id = allocate_record_id(&write)?;
    let record = StoredRecord {
        message_id: message.message_id,
        source: message.source,
        destination: message.destination,
        timestamp_unix_ms: message.timestamp_unix_ms,
        title: message.title,
        content: message.content,
        exact_wire: message.exact_wire,
        record_kind: StoredRecordKind::Outbound {
            generation: 1,
            failed_attempts: 0,
            state: StoredOutboundState::Queued,
        },
    };
    validate_record(local_record_id, &record)?;
    write_record(&write, local_record_id, &record)?;
    let revision = increment_revision(&write)?;
    commit(write, "insert outbound LXMF message")?;
    Ok(MailboxReply::OutboundInserted {
        message: public_message(local_record_id, record),
        revision,
    })
}

fn insert_inbound(
    database: &Database,
    message: NewInboundMessage,
) -> Result<MailboxReply, MailboxFailure> {
    let write = begin_write(database, "insert inbound LXMF message")?;
    let duplicate = {
        let index = write
            .open_table(LXMF_INBOUND_IDS)
            .map_err(|error| classify_redb_error(error.into(), "open LXMF inbound index"))?;
        let found = index
            .get(message.message_id.as_slice())
            .map_err(|error| classify_redb_error(error.into(), "read LXMF inbound index"))?
            .map(|value| value.value());
        found
    };
    if let Some(local_record_id) = duplicate {
        let Some(mut record) = read_record(&write, local_record_id)? else {
            return Err(MailboxFailure::reset_required(
                "an LXMF inbound index entry refers to a missing record",
            ));
        };
        let StoredRecordKind::Inbound { verification, .. } = record.record_kind else {
            return Err(MailboxFailure::reset_required(
                "an LXMF inbound index entry refers to an outbound record",
            ));
        };
        if record.message_id != message.message_id {
            return Err(MailboxFailure::reset_required(
                "an LXMF inbound index entry does not match its record",
            ));
        }
        if verification != StoredVerification::Verified
            && message.verification == LxmfVerification::Verified
        {
            // The message ID excludes the signature. An unverified first copy
            // must not permanently suppress a later source-verified receipt.
            // Retain the original local ID and require the exact signed body,
            // including fields not projected into title/content, to agree.
            let mut upgraded = record.clone();
            upgraded.exact_wire = message.exact_wire;
            upgraded.record_kind = StoredRecordKind::Inbound {
                verification: StoredVerification::Verified,
                verification_upgraded: true,
            };
            validate_record(local_record_id, &upgraded)?;
            if record.source != message.source
                || record.destination != message.destination
                || record.timestamp_unix_ms != message.timestamp_unix_ms
                || record.title != message.title
                || record.content != message.content
                || record.exact_wire[WIRE_HEADER_LENGTH..]
                    != upgraded.exact_wire[WIRE_HEADER_LENGTH..]
            {
                return Err(MailboxFailure::unavailable(
                    "the verified LXMF duplicate does not match the stored message",
                ));
            }
            record = upgraded;
            write_record(&write, local_record_id, &record)?;
            let revision = increment_revision(&write)?;
            commit(write, "upgrade inbound LXMF verification")?;
            return Ok(MailboxReply::Inbound(
                InboundInsertOutcome::VerificationUpgraded {
                    message: public_message(local_record_id, record),
                    revision,
                },
            ));
        }
        return Ok(MailboxReply::Inbound(InboundInsertOutcome::Duplicate {
            local_record_id,
        }));
    }

    let local_record_id = allocate_record_id(&write)?;
    let message_id = message.message_id;
    let record = StoredRecord {
        message_id,
        source: message.source,
        destination: message.destination,
        timestamp_unix_ms: message.timestamp_unix_ms,
        title: message.title,
        content: message.content,
        exact_wire: message.exact_wire,
        record_kind: StoredRecordKind::Inbound {
            verification: stored_verification(message.verification),
            verification_upgraded: false,
        },
    };
    validate_record(local_record_id, &record)?;
    write_record(&write, local_record_id, &record)?;
    {
        let mut index = write
            .open_table(LXMF_INBOUND_IDS)
            .map_err(|error| classify_redb_error(error.into(), "open LXMF inbound index"))?;
        index
            .insert(message_id.as_slice(), local_record_id)
            .map_err(|error| classify_redb_error(error.into(), "write LXMF inbound index"))?;
    }
    let revision = increment_revision(&write)?;
    commit(write, "insert inbound LXMF message")?;
    Ok(MailboxReply::Inbound(InboundInsertOutcome::Inserted {
        message: public_message(local_record_id, record),
        revision,
    }))
}

fn complete_attempt(
    database: &Database,
    key: AttemptKey,
    completion: AttemptCompletion,
) -> Result<MailboxReply, MailboxFailure> {
    let write = begin_write(database, "complete LXMF attempt")?;
    let Some(mut record) = read_record(&write, key.local_record_id)? else {
        return Ok(MailboxReply::Completion(CompletionTransition::Stale {
            current: None,
        }));
    };
    let StoredRecordKind::Outbound {
        generation,
        failed_attempts,
        state,
    } = &mut record.record_kind
    else {
        return Ok(MailboxReply::Completion(CompletionTransition::Stale {
            current: Some(DurableLxmfDeliveryState::Received),
        }));
    };
    if *generation != key.generation || !matches!(state, StoredOutboundState::Queued) {
        return Ok(MailboxReply::Completion(CompletionTransition::Stale {
            current: Some(public_delivery_state(*failed_attempts, *state)),
        }));
    }
    *state = match completion {
        AttemptCompletion::Delivered {
            delivered_at_millis,
            rtt_millis,
        } => StoredOutboundState::Delivered {
            delivered_at_millis,
            rtt_millis: RequiredNullableU64(rtt_millis),
        },
        AttemptCompletion::Failed { failure } => {
            *failed_attempts = failed_attempts.checked_add(1).ok_or_else(|| {
                MailboxFailure::unavailable("the LXMF failure counter is exhausted")
            })?;
            StoredOutboundState::Failed {
                last_failure: stored_failure(failure),
            }
        }
    };
    write_record(&write, key.local_record_id, &record)?;
    let revision = increment_revision(&write)?;
    commit(write, "complete LXMF attempt")?;
    Ok(MailboxReply::Completion(CompletionTransition::Applied {
        message: public_message(key.local_record_id, record),
        revision,
    }))
}

fn retry(database: &Database, local_record_id: u64) -> Result<MailboxReply, MailboxFailure> {
    let write = begin_write(database, "retry LXMF message")?;
    let Some(mut record) = read_record(&write, local_record_id)? else {
        return Ok(MailboxReply::Retry(RetryTransition::NotFound));
    };
    let StoredRecordKind::Outbound {
        generation,
        failed_attempts,
        state,
    } = &mut record.record_kind
    else {
        return Ok(MailboxReply::Retry(RetryTransition::NotFailed {
            current: DurableLxmfDeliveryState::Received,
        }));
    };
    if !matches!(state, StoredOutboundState::Failed { .. }) {
        return Ok(MailboxReply::Retry(RetryTransition::NotFailed {
            current: public_delivery_state(*failed_attempts, *state),
        }));
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| MailboxFailure::unavailable("the LXMF attempt generation is exhausted"))?;
    *state = StoredOutboundState::Queued;
    write_record(&write, local_record_id, &record)?;
    let revision = increment_revision(&write)?;
    commit(write, "retry LXMF message")?;
    Ok(MailboxReply::Retry(RetryTransition::Accepted {
        message: public_message(local_record_id, record),
        revision,
    }))
}

fn cancel(
    database: &Database,
    local_record_id: u64,
    cancelled_at_millis: u64,
) -> Result<MailboxReply, MailboxFailure> {
    let write = begin_write(database, "cancel LXMF message")?;
    let Some(mut record) = read_record(&write, local_record_id)? else {
        return Ok(MailboxReply::Cancel(CancelTransition::NotFound));
    };
    let StoredRecordKind::Outbound {
        failed_attempts,
        state,
        ..
    } = &mut record.record_kind
    else {
        return Ok(MailboxReply::Cancel(CancelTransition::NotCancellable {
            current: DurableLxmfDeliveryState::Received,
        }));
    };
    match *state {
        StoredOutboundState::Queued => {}
        StoredOutboundState::Delivered { .. } => {
            return Ok(MailboxReply::Cancel(CancelTransition::AlreadyDelivered));
        }
        StoredOutboundState::Cancelled { .. } => {
            return Ok(MailboxReply::Cancel(CancelTransition::AlreadyCancelled));
        }
        StoredOutboundState::Failed { .. } => {
            return Ok(MailboxReply::Cancel(CancelTransition::NotCancellable {
                current: public_delivery_state(*failed_attempts, *state),
            }));
        }
    }
    *state = StoredOutboundState::Cancelled {
        cancelled_at_millis,
    };
    write_record(&write, local_record_id, &record)?;
    let revision = increment_revision(&write)?;
    commit(write, "cancel LXMF message")?;
    Ok(MailboxReply::Cancel(CancelTransition::Cancelled {
        message: public_message(local_record_id, record),
        revision,
    }))
}

fn list(database: &Database, request: MailboxListRequest) -> Result<MailboxReply, MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "list LXMF messages"))?;
    let meta = read
        .open_table(LXMF_META)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF metadata"))?;
    let revision = required_meta(&meta, LXMF_REVISION_KEY)?;
    drop(meta);
    let table = read
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let selection = policy::read(&read)?;
    let capacity = policy::page_capacity(selection.limits, request.limit)?;
    let mut messages = policy::allocate_page(selection.limits, capacity)?;
    if request.limit == 0 {
        return Ok(MailboxReply::Listed { messages, revision });
    }
    // Redb owns the cursor's page storage. Deserialize only records actually
    // visited before reaching the requested number of matching messages.
    for entry in table
        .range((
            Bound::Unbounded,
            request.before.map_or(Bound::Unbounded, Bound::Excluded),
        ))
        .map_err(|error| classify_redb_error(error.into(), "scan LXMF message table"))?
        .rev()
    {
        let (key, value) =
            entry.map_err(|error| classify_redb_error(error.into(), "scan LXMF message record"))?;
        let local_record_id = key.value();
        if local_record_id == 0 {
            continue;
        }
        let record = decode_record(local_record_id, value.value())?;
        let message = public_message(local_record_id, record);
        if let Some(direction) = request.direction {
            let matches = matches!(
                (direction, message.direction),
                (MailboxDirectionFilter::Inbox, LxmfDirection::Inbound)
                    | (MailboxDirectionFilter::Outbox, LxmfDirection::Outbound)
            );
            if !matches {
                continue;
            }
        }
        if let Some(peer) = request.peer {
            if message.source != peer && message.destination != peer {
                continue;
            }
        }
        messages.push(message);
        if messages.len() == capacity {
            break;
        }
    }
    policy::check_page(selection.limits, &messages)?;
    Ok(MailboxReply::Listed { messages, revision })
}

fn list_conversations(
    database: &Database,
    request: MailboxConversationListRequest,
) -> Result<MailboxReply, MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "list LXMF conversations"))?;
    let meta = read
        .open_table(LXMF_META)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF metadata"))?;
    let revision = required_meta(&meta, LXMF_REVISION_KEY)?;
    let selection = policy::read(&read)?;
    let capacity = policy::page_capacity(selection.limits, request.limit)?;
    let mut messages = policy::allocate_page(selection.limits, capacity)?;
    if request.limit == 0 {
        return Ok(MailboxReply::ConversationsListed { messages, revision });
    }
    let table = read
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let mut seen = BTreeSet::new();
    for entry in table
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan LXMF conversations"))?
        .rev()
    {
        let (key, value) =
            entry.map_err(|error| classify_redb_error(error.into(), "scan LXMF conversation"))?;
        let local_record_id = key.value();
        if local_record_id == 0 {
            continue;
        }
        let record = decode_record(local_record_id, value.value())?;
        let peer = match record.record_kind {
            StoredRecordKind::Inbound { .. } => record.source,
            StoredRecordKind::Outbound { .. } => record.destination,
        };
        // Remember the newest record even when it is ahead of the cursor. An
        // older message must never make that conversation appear a second time.
        if !seen.insert(peer)
            || request
                .before
                .is_some_and(|before| local_record_id >= before)
        {
            continue;
        }
        messages.push(public_message(local_record_id, record));
        if messages.len() == capacity {
            break;
        }
    }
    policy::check_page(selection.limits, &messages)?;
    Ok(MailboxReply::ConversationsListed { messages, revision })
}

fn begin_write(
    database: &Database,
    operation: &str,
) -> Result<redb::WriteTransaction, MailboxFailure> {
    let write = database
        .begin_write()
        .map_err(|e| classify_redb_error(e.into(), operation))?;
    {
        let meta = write
            .open_table(LXMF_META)
            .map_err(|e| classify_redb_error(e.into(), "open mailbox metadata"))?;
        let header = policy::Header::read(&meta)?;
        let rows = write
            .open_table(LXMF_MESSAGES)
            .map_err(|e| classify_redb_error(e.into(), "open mailbox rows"))?;
        let ids = write
            .open_table(LXMF_INBOUND_IDS)
            .map_err(|e| classify_redb_error(e.into(), "open mailbox IDs"))?;
        let queued = write
            .open_table(policy::QUEUED)
            .map_err(|e| classify_redb_error(e.into(), "open mailbox queue"))?;
        policy::check_markers(&rows, &ids, &queued)?;
        let expected = header
            .records
            .checked_add(1)
            .ok_or(MailboxFailure::Capacity)?;
        if required_meta(&meta, NEXT_LXMF_RECORD_ID_KEY)? != expected
            || rows
                .len()
                .map_err(|e| classify_redb_error(e.into(), "count mailbox rows"))?
                != expected
        {
            return Err(MailboxFailure::reset_required(
                "mailbox header/row count mismatch",
            ));
        }
    }
    // Any missing table marker above drops this owned transaction. Redb's
    // write-open cannot escape as a committed replacement empty schema.
    Ok(write)
}

fn commit(write: redb::WriteTransaction, operation: &str) -> Result<(), MailboxFailure> {
    write
        .commit()
        .map_err(|error| classify_redb_error(error.into(), operation))
}

fn allocate_record_id(write: &redb::WriteTransaction) -> Result<u64, MailboxFailure> {
    let mut meta = write
        .open_table(LXMF_META)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF metadata"))?;
    let next = required_meta(&meta, NEXT_LXMF_RECORD_ID_KEY)?;
    if next == 0 {
        return Err(MailboxFailure::reset_required(
            "the next LXMF record ID uses reserved value zero",
        ));
    }
    let following = next.checked_add(1).ok_or_else(|| {
        MailboxFailure::unavailable("the LXMF local record ID space is exhausted")
    })?;
    meta.insert(NEXT_LXMF_RECORD_ID_KEY, following)
        .map_err(|error| classify_redb_error(error.into(), "advance next LXMF record ID"))?;
    Ok(next)
}

fn increment_revision(write: &redb::WriteTransaction) -> Result<u64, MailboxFailure> {
    let mut meta = write
        .open_table(LXMF_META)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF metadata"))?;
    let current = required_meta(&meta, LXMF_REVISION_KEY)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| MailboxFailure::unavailable("the LXMF durable revision is exhausted"))?;
    meta.insert(LXMF_REVISION_KEY, next)
        .map_err(|error| classify_redb_error(error.into(), "advance LXMF durable revision"))?;
    Ok(next)
}

fn required_meta(
    table: &impl ReadableTable<&'static str, u64>,
    key: &str,
) -> Result<u64, MailboxFailure> {
    table
        .get(key)
        .map_err(|error| classify_redb_error(error.into(), "read LXMF metadata"))?
        .map(|value| value.value())
        .ok_or_else(|| {
            MailboxFailure::reset_required(format!("LXMF metadata key {key:?} is missing"))
        })
}

fn read_record(
    write: &redb::WriteTransaction,
    local_record_id: u64,
) -> Result<Option<StoredRecord>, MailboxFailure> {
    if local_record_id == 0 {
        return Ok(None);
    }
    let table = write
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let decoded = table
        .get(local_record_id)
        .map_err(|e| classify_redb_error(e.into(), "read LXMF message record"))?
        .map(|encoded| decode_record(local_record_id, encoded.value()))
        .transpose()?;
    Ok(decoded)
}

fn write_record(
    write: &redb::WriteTransaction,
    local_record_id: u64,
    record: &StoredRecord,
) -> Result<(), MailboxFailure> {
    let old = read_record(write, local_record_id)?;
    let encoded = policy::encode(record)?;
    policy::stage(write, local_record_id, old.as_ref(), record)?;
    let mut table = write
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    table
        .insert(local_record_id, encoded.as_slice())
        .map(|_| ())
        .map_err(|error| classify_redb_error(error.into(), "write LXMF message record"))
}

fn decode_record(local_record_id: u64, encoded: &[u8]) -> Result<StoredRecord, MailboxFailure> {
    if local_record_id == 0 || encoded.len() > policy::maximum_encoded_bytes()? {
        return Err(MailboxFailure::reset_required("mailbox encoded row bound"));
    }
    let record: StoredRecord = serde_json::from_slice(encoded).map_err(|error| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} is malformed: {error}"
        ))
    })?;
    if [&record.title, &record.content, &record.exact_wire]
        .iter()
        .any(|bytes| bytes.capacity() > MAX_BASIC_LXMF_WIRE_BYTES)
    {
        return Err(MailboxFailure::reset_required(
            "mailbox decoded allocation bound",
        ));
    }
    validate_record(local_record_id, &record)?;
    Ok(record)
}

fn validate_record(local_record_id: u64, record: &StoredRecord) -> Result<(), MailboxFailure> {
    let limits = direct_wire_limits();
    let message = MessageView::parse_complete(&record.exact_wire, limits).map_err(|error| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} holds invalid exact wire: {error:?}"
        ))
    })?;
    if message.payload().stamp().is_some() {
        return Err(MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} contains an unsupported stamped payload"
        )));
    }
    let timestamp_unix_ms = timestamp_millis(message.payload().timestamp()).ok_or_else(|| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} contains an invalid timestamp"
        ))
    })?;
    if message.message_id() != record.message_id
        || message.source_hash() != &record.source
        || message.destination_hash() != &record.destination
        || timestamp_unix_ms != record.timestamp_unix_ms
        || message.payload().title().as_bytes() != record.title
        || message.payload().content().as_bytes() != record.content
    {
        return Err(MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} does not match its exact wire"
        )));
    }
    let _mutation_count = record_mutation_count(local_record_id, &record.record_kind)?;
    Ok(())
}

fn record_mutation_count(
    local_record_id: u64,
    kind: &StoredRecordKind,
) -> Result<u64, MailboxFailure> {
    if let StoredRecordKind::Inbound {
        verification,
        verification_upgraded,
    } = kind
    {
        if *verification_upgraded && *verification != StoredVerification::Verified {
            return Err(MailboxFailure::reset_required(format!(
                "LXMF record {local_record_id} has an unverified verification upgrade"
            )));
        }
        return Ok(1 + u64::from(*verification_upgraded));
    }
    let StoredRecordKind::Outbound {
        generation,
        failed_attempts,
        state,
    } = kind
    else {
        return Ok(1);
    };
    let expected_generation = match state {
        StoredOutboundState::Failed { .. } => {
            if *failed_attempts == 0 {
                return Err(MailboxFailure::reset_required(format!(
                    "failed LXMF record {local_record_id} has no failed attempts"
                )));
            }
            *failed_attempts
        }
        StoredOutboundState::Queued
        | StoredOutboundState::Delivered { .. }
        | StoredOutboundState::Cancelled { .. } => {
            failed_attempts.checked_add(1).ok_or_else(|| {
                MailboxFailure::reset_required(format!(
                    "LXMF record {local_record_id} has exhausted its attempt generation"
                ))
            })?
        }
    };
    if *generation != expected_generation {
        return Err(MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} has generation {generation}, expected {expected_generation}"
        )));
    }
    let completed_cycles = failed_attempts.checked_mul(2).ok_or_else(|| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} has exhausted its mutation count"
        ))
    })?;
    let offset = match state {
        StoredOutboundState::Failed { .. } => 0,
        StoredOutboundState::Queued => 1,
        StoredOutboundState::Delivered { .. } | StoredOutboundState::Cancelled { .. } => 2,
    };
    completed_cycles.checked_add(offset).ok_or_else(|| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} has exhausted its mutation count"
        ))
    })
}

fn direct_wire_limits() -> WireLimits {
    WireLimits::new(
        MAX_BASIC_LXMF_WIRE_BYTES,
        MAX_BASIC_LXMF_WIRE_BYTES,
        128,
        512,
        8_192,
        16,
    )
}

fn timestamp_millis(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let millis = seconds * 1_000.0;
    (millis <= u64::MAX as f64).then_some(millis as u64)
}

fn public_message(local_record_id: u64, record: StoredRecord) -> DurableLxmfMessage {
    let (direction, verification, delivery_state, generation) = match record.record_kind {
        StoredRecordKind::Inbound { verification, .. } => (
            LxmfDirection::Inbound,
            public_verification(verification),
            DurableLxmfDeliveryState::Received,
            None,
        ),
        StoredRecordKind::Outbound {
            generation,
            failed_attempts,
            state,
        } => (
            LxmfDirection::Outbound,
            LxmfVerification::Verified,
            public_delivery_state(failed_attempts, state),
            Some(generation),
        ),
    };
    DurableLxmfMessage {
        local_record_id,
        message_id: record.message_id,
        source: record.source,
        destination: record.destination,
        timestamp_unix_ms: record.timestamp_unix_ms,
        title: record.title,
        content: record.content,
        exact_wire: record.exact_wire,
        direction,
        verification,
        delivery_state,
        generation,
    }
}

const fn public_delivery_state(
    failed_attempts: u64,
    state: StoredOutboundState,
) -> DurableLxmfDeliveryState {
    match state {
        StoredOutboundState::Queued => DurableLxmfDeliveryState::Queued { failed_attempts },
        StoredOutboundState::Delivered {
            delivered_at_millis,
            rtt_millis,
        } => DurableLxmfDeliveryState::Delivered {
            delivered_at_millis,
            rtt_millis: rtt_millis.0,
        },
        StoredOutboundState::Failed { last_failure } => DurableLxmfDeliveryState::Failed {
            failed_attempts,
            last_failure: public_failure(last_failure),
        },
        StoredOutboundState::Cancelled {
            cancelled_at_millis,
        } => DurableLxmfDeliveryState::Cancelled {
            cancelled_at_millis,
        },
    }
}

const fn stored_verification(verification: LxmfVerification) -> StoredVerification {
    match verification {
        LxmfVerification::Verified => StoredVerification::Verified,
        LxmfVerification::SourceUnknown => StoredVerification::SourceUnknown,
        LxmfVerification::InvalidSignature => StoredVerification::InvalidSignature,
    }
}

const fn public_verification(verification: StoredVerification) -> LxmfVerification {
    match verification {
        StoredVerification::Verified => LxmfVerification::Verified,
        StoredVerification::SourceUnknown => LxmfVerification::SourceUnknown,
        StoredVerification::InvalidSignature => LxmfVerification::InvalidSignature,
    }
}

const fn stored_failure(failure: DirectSendFailure) -> StoredFailure {
    match failure {
        DirectSendFailure::NoRoute => StoredFailure::NoRoute,
        DirectSendFailure::LinkFailed => StoredFailure::LinkFailed,
        DirectSendFailure::DeliveryTimedOut => StoredFailure::DeliveryTimedOut,
        DirectSendFailure::LocalNodeStopped => StoredFailure::LocalNodeStopped,
    }
}

const fn public_failure(failure: StoredFailure) -> DirectSendFailure {
    match failure {
        StoredFailure::NoRoute => DirectSendFailure::NoRoute,
        StoredFailure::LinkFailed => DirectSendFailure::LinkFailed,
        StoredFailure::DeliveryTimedOut => DirectSendFailure::DeliveryTimedOut,
        StoredFailure::LocalNodeStopped => DirectSendFailure::LocalNodeStopped,
    }
}

fn classify_redb_error(error: redb::Error, operation: &str) -> MailboxFailure {
    let detail = format!("could not {operation}: {error}");
    match error {
        redb::Error::Corrupted(_)
        | redb::Error::UpgradeRequired(_)
        | redb::Error::RepairAborted
        | redb::Error::TableTypeMismatch { .. }
        | redb::Error::TableIsMultimap(_)
        | redb::Error::TableIsNotMultimap(_)
        | redb::Error::TypeDefinitionChanged { .. }
        | redb::Error::TableDoesNotExist(_) => MailboxFailure::reset_required(detail),
        redb::Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            MailboxFailure::reset_required(detail)
        }
        _ => MailboxFailure::unavailable(detail),
    }
}

#[cfg(feature = "tokio-runtime")]
mod runtime;
#[cfg(feature = "tokio-runtime")]
pub use runtime::*;

mod policy;
pub use policy::MailboxLimits;
mod replay;
pub use replay::MailboxReplayRequest;

#[cfg(test)]
mod tests;
