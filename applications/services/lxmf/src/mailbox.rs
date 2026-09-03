//! Durable direct-message records and transactions for an application-owned `redb`.
//!
//! This module never opens or owns a database. The application that already owns
//! its database submits [`MailboxRequest`] values to its one blocking owner and
//! calls [`execute_mailbox_request`] there.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::string::String;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::vec::Vec;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::direct::{
    run_single_attempt, DirectAnnounceFailure, DirectNetwork, DirectSendFailure,
    DirectServiceStopError, Job, LaneHealth, LocalLxmfIdentity, LxmfCallbacks, LxmfPeer,
    DIRECT_JOB_CAPACITY, MAX_DISPLAY_NAME_BYTES, STOP_JOIN_TIMEOUT,
};
use crate::{LxmfDirection, LxmfVerification};
use prns_lxmf_wire::{
    compose_basic_direct_lxmf, normalize_lxmf_display_name, parse_lxmf_announce,
    BasicLxmfComposeError, CarrierIngress, MessageView, WireLimits, MAX_BASIC_LXMF_WIRE_BYTES,
};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;

/// Existing application metadata table shared with the directory owner.
pub const DEVELOPMENT_META: TableDefinition<&str, u64> = TableDefinition::new("development_meta");
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

const SETTLEMENT_RETRY_DELAY: Duration = Duration::from_millis(25);

/// A boxed request future implemented by the application's one database owner.
pub type MailboxFuture<'a> =
    Pin<Box<dyn Future<Output = Result<MailboxReply, MailboxFailure>> + Send + 'a>>;

/// Narrow command port into the application's existing database owner.
///
/// Implementations must not open another database. A native aggregate typically
/// implements this by placing the request on the same bounded lane used by its
/// contact operations.
pub trait MailboxSubmitter: Send + Sync + 'static {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_>;
}

/// Failure classification preserved through the aggregate boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxFailure {
    Busy,
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
    pub(crate) const fn generation(&self) -> Option<u64> {
        self.generation
    }

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

/// Development-scale list direction filter.
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

/// Closed mailbox request language executed on the application's owner thread.
#[derive(Debug)]
pub enum MailboxRequest {
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
    ListQueued,
}

/// One request's typed result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxReply {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundInsertOutcome {
    Inserted {
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
    title: Vec<u8>,
    content: Vec<u8>,
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
    },
    Outbound {
        generation: u64,
        failed_attempts: u64,
        state: StoredOutboundState,
    },
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
    {
        let mut meta = write
            .open_table(DEVELOPMENT_META)
            .map_err(|error| classify_redb_error(error.into(), "open development metadata"))?;
        meta.insert(NEXT_LXMF_RECORD_ID_KEY, INITIAL_NEXT_LXMF_RECORD_ID)
            .map_err(|error| classify_redb_error(error.into(), "initialize next LXMF record ID"))?;
        meta.insert(LXMF_REVISION_KEY, INITIAL_LXMF_REVISION)
            .map_err(|error| classify_redb_error(error.into(), "initialize LXMF revision"))?;
    }
    write
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "create LXMF message table"))?;
    write
        .open_table(LXMF_INBOUND_IDS)
        .map_err(|error| classify_redb_error(error.into(), "create LXMF inbound index"))?;
    Ok(())
}

/// Validate all mailbox tables, counters, records, and index relationships.
pub fn validate_mailbox_database(database: &Database) -> Result<(), MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "read development mailbox"))?;
    let meta = read
        .open_table(DEVELOPMENT_META)
        .map_err(|error| classify_redb_error(error.into(), "open development metadata"))?;
    let next_id = required_meta(&meta, NEXT_LXMF_RECORD_ID_KEY)?;
    let revision = required_meta(&meta, LXMF_REVISION_KEY)?;
    drop(meta);

    let messages = read
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let mut expected_id = INITIAL_NEXT_LXMF_RECORD_ID;
    let mut expected_revision = INITIAL_LXMF_REVISION;
    let mut inbound = BTreeMap::new();
    for entry in messages
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan LXMF message table"))?
    {
        let (key, value) =
            entry.map_err(|error| classify_redb_error(error.into(), "scan LXMF message record"))?;
        let local_record_id = key.value();
        if local_record_id != expected_id {
            return Err(MailboxFailure::reset_required(format!(
                "LXMF record ID {local_record_id} is out of sequence; expected {expected_id}"
            )));
        }
        expected_id = expected_id.checked_add(1).ok_or_else(|| {
            MailboxFailure::reset_required("the LXMF local record ID space is exhausted")
        })?;
        let record = decode_record(local_record_id, value.value())?;
        expected_revision = expected_revision
            .checked_add(record_mutation_count(local_record_id, &record.record_kind)?)
            .ok_or_else(|| {
                MailboxFailure::reset_required("the LXMF durable revision is exhausted")
            })?;
        if matches!(record.record_kind, StoredRecordKind::Inbound { .. })
            && inbound.insert(record.message_id, local_record_id).is_some()
        {
            return Err(MailboxFailure::reset_required(
                "two inbound LXMF records share one message ID",
            ));
        }
    }
    drop(messages);

    if next_id != expected_id {
        return Err(MailboxFailure::reset_required(format!(
            "the next LXMF record ID is {next_id}, expected {expected_id}"
        )));
    }
    if revision != expected_revision {
        return Err(MailboxFailure::reset_required(format!(
            "the LXMF durable revision is {revision}, expected {expected_revision}"
        )));
    }

    let index = read
        .open_table(LXMF_INBOUND_IDS)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF inbound index"))?;
    let mut indexed = 0_usize;
    for entry in index
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan LXMF inbound index"))?
    {
        let (key, value) = entry
            .map_err(|error| classify_redb_error(error.into(), "scan LXMF inbound index entry"))?;
        let message_id: [u8; 32] = key.value().try_into().map_err(|_| {
            MailboxFailure::reset_required(format!(
                "an LXMF inbound index key holds {} bytes instead of 32",
                key.value().len()
            ))
        })?;
        let local_record_id = value.value();
        if inbound.get(&message_id) != Some(&local_record_id) {
            return Err(MailboxFailure::reset_required(
                "an LXMF inbound index entry does not match its message record",
            ));
        }
        indexed = indexed.checked_add(1).ok_or_else(|| {
            MailboxFailure::reset_required("the LXMF inbound index count overflowed")
        })?;
    }
    if indexed != inbound.len() {
        return Err(MailboxFailure::reset_required(
            "an inbound LXMF message is missing its deduplication index entry",
        ));
    }
    Ok(())
}

/// Execute one complete mailbox operation on the calling database owner thread.
pub fn execute_mailbox_request(
    database: &Database,
    request: MailboxRequest,
) -> Result<MailboxReply, MailboxFailure> {
    match request {
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
        MailboxRequest::List(request) => list(database, request, false),
        MailboxRequest::ListQueued => list(
            database,
            MailboxListRequest {
                peer: None,
                direction: Some(MailboxDirectionFilter::Outbox),
                before: None,
                limit: usize::MAX,
            },
            true,
        ),
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

fn list(
    database: &Database,
    request: MailboxListRequest,
    queued_only: bool,
) -> Result<MailboxReply, MailboxFailure> {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "list LXMF messages"))?;
    let meta = read
        .open_table(DEVELOPMENT_META)
        .map_err(|error| classify_redb_error(error.into(), "open development metadata"))?;
    let revision = required_meta(&meta, LXMF_REVISION_KEY)?;
    drop(meta);
    let table = read
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let mut records = Vec::new();
    for entry in table
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan LXMF message table"))?
    {
        let (key, value) =
            entry.map_err(|error| classify_redb_error(error.into(), "scan LXMF message record"))?;
        records.push((key.value(), value.value().to_vec()));
    }
    let mut messages = Vec::new();
    for (local_record_id, encoded) in records.into_iter().rev() {
        if request
            .before
            .is_some_and(|before| local_record_id >= before)
        {
            continue;
        }
        let record = decode_record(local_record_id, &encoded)?;
        let message = public_message(local_record_id, record);
        if queued_only
            && !matches!(
                message.delivery_state,
                DurableLxmfDeliveryState::Queued { .. }
            )
        {
            continue;
        }
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
        if messages.len() >= request.limit {
            break;
        }
        messages.push(message);
    }
    Ok(MailboxReply::Listed { messages, revision })
}

fn begin_write(
    database: &Database,
    operation: &str,
) -> Result<redb::WriteTransaction, MailboxFailure> {
    database
        .begin_write()
        .map_err(|error| classify_redb_error(error.into(), operation))
}

fn commit(write: redb::WriteTransaction, operation: &str) -> Result<(), MailboxFailure> {
    write
        .commit()
        .map_err(|error| classify_redb_error(error.into(), operation))
}

fn allocate_record_id(write: &redb::WriteTransaction) -> Result<u64, MailboxFailure> {
    let mut meta = write
        .open_table(DEVELOPMENT_META)
        .map_err(|error| classify_redb_error(error.into(), "open development metadata"))?;
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
        .open_table(DEVELOPMENT_META)
        .map_err(|error| classify_redb_error(error.into(), "open development metadata"))?;
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
    let table = write
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    let encoded = table
        .get(local_record_id)
        .map_err(|error| classify_redb_error(error.into(), "read LXMF message record"))?
        .map(|value| value.value().to_vec());
    encoded
        .map(|encoded| decode_record(local_record_id, &encoded))
        .transpose()
}

fn write_record(
    write: &redb::WriteTransaction,
    local_record_id: u64,
    record: &StoredRecord,
) -> Result<(), MailboxFailure> {
    let encoded = serde_json::to_vec(record).map_err(|error| {
        MailboxFailure::unavailable(format!("could not encode LXMF message record: {error}"))
    })?;
    let mut table = write
        .open_table(LXMF_MESSAGES)
        .map_err(|error| classify_redb_error(error.into(), "open LXMF message table"))?;
    table
        .insert(local_record_id, encoded.as_slice())
        .map(|_| ())
        .map_err(|error| classify_redb_error(error.into(), "write LXMF message record"))
}

fn decode_record(local_record_id: u64, encoded: &[u8]) -> Result<StoredRecord, MailboxFailure> {
    let record: StoredRecord = serde_json::from_slice(encoded).map_err(|error| {
        MailboxFailure::reset_required(format!(
            "LXMF record {local_record_id} is malformed: {error}"
        ))
    })?;
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
        StoredRecordKind::Inbound { verification } => (
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

/// Wall-clock source retained outside the durable state machine for deterministic tests.
pub trait MailboxClock: Send + Sync + 'static {
    fn now_unix_millis(&self) -> u64;
}

/// System wall clock used by production composition.
#[derive(Debug, Default)]
pub struct SystemMailboxClock;

impl MailboxClock for SystemMailboxClock {
    fn now_unix_millis(&self) -> u64 {
        let elapsed = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => elapsed,
            Err(_) => Duration::ZERO,
        };
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }
}

/// Storage-specific health that complements the existing callback-lane health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxProjectionHealth {
    Ready,
    Busy,
    Unavailable(String),
    ResetRequired(String),
}

/// Authoritative durable snapshot plus process-local peer and active-attempt projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableLxmfSnapshot {
    pub peers: Vec<LxmfPeer>,
    pub messages: Vec<DurableLxmfMessage>,
    pub health: crate::LxmfHealth,
    pub mailbox_health: MailboxProjectionHealth,
    pub durable_revision: u64,
    pub projection_revision: u64,
}

/// LXM3 submission returns after the queued transaction, before network settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableSendDirectTextOutcome {
    Accepted { local_record_id: u64 },
    NeedsResource { wire_bytes: usize },
    UnsupportedRemoteStampRequirement { required_stamp_cost: u64 },
    PeerIdentityUnavailable,
    LocalNodeStopped,
    Busy,
    InvalidMessage,
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryLxmfMessageOutcome {
    Accepted { local_record_id: u64 },
    NotFound,
    NotFailed { current: DurableLxmfDeliveryState },
    LocalNodeStopped,
    Busy,
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelLxmfMessageOutcome {
    Cancelled { local_record_id: u64 },
    NotFound,
    AlreadyDelivered,
    AlreadyCancelled,
    NotCancellable { current: DurableLxmfDeliveryState },
    LocalNodeStopped,
    Busy,
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

/// Failure to start a durable direct service generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableDirectServiceStartError {
    NoTokioRuntime,
    Mailbox(MailboxFailure),
    UnexpectedMailboxReply,
}

/// Callback lane prepared before the owning aggregate constructs its PRNS node.
pub struct PendingDurableDirectLxmfService {
    identity: LocalLxmfIdentity,
    callbacks: LxmfCallbacks,
    receiver: mpsc::Receiver<Job>,
    refresh: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    shutdown_receiver: watch::Receiver<bool>,
}

/// One durable direct LXMF generation.
#[derive(Clone)]
pub struct DurableDirectLxmfService {
    shared: Arc<DurableShared>,
    lifecycle: Arc<DurableLifecycle>,
    callbacks: LxmfCallbacks,
}

struct DurableShared {
    local_destination: [u8; 16],
    identity: Arc<LocalLxmfIdentity>,
    network: Arc<dyn DirectNetwork>,
    mailbox: Arc<dyn MailboxSubmitter>,
    clock: Arc<dyn MailboxClock>,
    peers: Mutex<BTreeMap<[u8; 16], LxmfPeer>>,
    health: Arc<LaneHealth>,
    mailbox_health: StdMutex<MailboxProjectionHealth>,
    refresh: watch::Sender<u64>,
}

struct DurableLifecycle {
    shutdown: watch::Sender<bool>,
    stopped: AtomicBool,
    stop_complete: AtomicBool,
    transition: Mutex<()>,
    worker: StdMutex<Option<JoinHandle<()>>>,
    attempts: StdMutex<BTreeMap<AttemptKey, JoinHandle<()>>>,
}

impl DurableLifecycle {
    fn worker(&self) -> StdMutexGuard<'_, Option<JoinHandle<()>>> {
        self.worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn attempts(&self) -> StdMutexGuard<'_, BTreeMap<AttemptKey, JoinHandle<()>>> {
        self.attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for DurableLifecycle {
    fn drop(&mut self) {
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            worker.abort();
        }
        let attempts = mem::take(
            self.attempts
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (_key, attempt) in attempts {
            attempt.abort();
        }
    }
}

impl DurableDirectLxmfService {
    /// Prepare callbacks before the aggregate constructs its one node.
    #[must_use]
    pub fn prepare(
        identity: LocalLxmfIdentity,
    ) -> (PendingDurableDirectLxmfService, LxmfCallbacks) {
        let local_destination = identity.destination();
        let (jobs, receiver) = mpsc::channel(DIRECT_JOB_CAPACITY);
        let (refresh, _refresh_receiver) = watch::channel(1_u64);
        let health = Arc::new(LaneHealth::new(refresh.clone()));
        let callbacks = LxmfCallbacks {
            local_destination,
            jobs,
            health,
        };
        let (shutdown, shutdown_receiver) = watch::channel(false);
        (
            PendingDurableDirectLxmfService {
                identity,
                callbacks: callbacks.clone(),
                receiver,
                refresh,
                shutdown,
                shutdown_receiver,
            },
            callbacks,
        )
    }

    #[must_use]
    pub fn callbacks(&self) -> LxmfCallbacks {
        self.callbacks.clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.shared.refresh.subscribe()
    }

    #[must_use]
    pub const fn local_destination(&self) -> [u8; 16] {
        self.callbacks.local_destination
    }

    pub async fn announce(&self) -> Result<(), DirectAnnounceFailure> {
        self.shared
            .network
            .announce(self.shared.local_destination)
            .await
    }

    /// Compose/sign once, commit `Queued`, register the attempt, then return `Accepted`.
    pub async fn send_direct_text(
        &self,
        destination: [u8; 16],
        timestamp_unix_ms: u64,
        title: &[u8],
        content: &[u8],
    ) -> DurableSendDirectTextOutcome {
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return DurableSendDirectTextOutcome::LocalNodeStopped;
        }
        let peer = self.shared.peers.lock().await.get(&destination).cloned();
        let Some(peer) = peer else {
            return DurableSendDirectTextOutcome::PeerIdentityUnavailable;
        };
        if let Some(required_stamp_cost) = peer.required_stamp_cost {
            return DurableSendDirectTextOutcome::UnsupportedRemoteStampRequirement {
                required_stamp_cost,
            };
        }
        let message = match compose_new_outbound(
            self.shared.local_destination,
            destination,
            timestamp_unix_ms,
            title,
            content,
            self.shared.identity.as_ref(),
        ) {
            Ok(message) => message,
            Err(BasicLxmfComposeError::NeedsResource { wire_bytes }) => {
                return DurableSendDirectTextOutcome::NeedsResource { wire_bytes };
            }
            Err(_) => return DurableSendDirectTextOutcome::InvalidMessage,
        };

        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return DurableSendDirectTextOutcome::LocalNodeStopped;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::InsertOutbound(message))
            .await
        {
            Ok(reply) => {
                self.shared.set_mailbox_ready();
                reply
            }
            Err(failure) => {
                self.shared.set_mailbox_failure(&failure);
                return send_failure_outcome(failure);
            }
        };
        let MailboxReply::OutboundInserted { message, .. } = reply else {
            return DurableSendDirectTextOutcome::DevelopmentUnavailable {
                detail: "the mailbox owner returned an unexpected insert reply".to_owned(),
            };
        };
        self.shared.notify();
        let local_record_id = message.local_record_id;
        if !self.lifecycle.stopped.load(Ordering::Acquire) {
            spawn_attempt(&self.shared, &self.lifecycle, message);
        }
        DurableSendDirectTextOutcome::Accepted { local_record_id }
    }

    /// Conditionally requeue the exact stored wire with a new attempt generation.
    pub async fn retry_lxmf_message(&self, local_record_id: u64) -> RetryLxmfMessageOutcome {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return RetryLxmfMessageOutcome::LocalNodeStopped;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::Retry { local_record_id })
            .await
        {
            Ok(reply) => {
                self.shared.set_mailbox_ready();
                reply
            }
            Err(failure) => {
                self.shared.set_mailbox_failure(&failure);
                return retry_failure_outcome(failure);
            }
        };
        let MailboxReply::Retry(transition) = reply else {
            return RetryLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "the mailbox owner returned an unexpected retry reply".to_owned(),
            };
        };
        match transition {
            RetryTransition::Accepted { message, .. } => {
                self.shared.notify();
                if !self.lifecycle.stopped.load(Ordering::Acquire) {
                    spawn_attempt(&self.shared, &self.lifecycle, message);
                }
                RetryLxmfMessageOutcome::Accepted { local_record_id }
            }
            RetryTransition::NotFound => RetryLxmfMessageOutcome::NotFound,
            RetryTransition::NotFailed { current } => {
                RetryLxmfMessageOutcome::NotFailed { current }
            }
        }
    }

    /// Conditionally commit cancellation before aborting the matching active generation.
    pub async fn cancel_lxmf_message(
        &self,
        local_record_id: u64,
        cancelled_at_millis: u64,
    ) -> CancelLxmfMessageOutcome {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return CancelLxmfMessageOutcome::LocalNodeStopped;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::Cancel {
                local_record_id,
                cancelled_at_millis,
            })
            .await
        {
            Ok(reply) => {
                self.shared.set_mailbox_ready();
                reply
            }
            Err(failure) => {
                self.shared.set_mailbox_failure(&failure);
                return cancel_failure_outcome(failure);
            }
        };
        let MailboxReply::Cancel(transition) = reply else {
            return CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "the mailbox owner returned an unexpected cancel reply".to_owned(),
            };
        };
        match transition {
            CancelTransition::Cancelled { message, .. } => {
                self.shared.notify();
                if let Some(generation) = message.generation() {
                    let key = AttemptKey {
                        local_record_id,
                        generation,
                    };
                    let attempt = self.lifecycle.attempts().remove(&key);
                    if let Some(attempt) = attempt {
                        attempt.abort();
                        self.shared.notify();
                    }
                }
                CancelLxmfMessageOutcome::Cancelled { local_record_id }
            }
            CancelTransition::NotFound => CancelLxmfMessageOutcome::NotFound,
            CancelTransition::AlreadyDelivered => CancelLxmfMessageOutcome::AlreadyDelivered,
            CancelTransition::AlreadyCancelled => CancelLxmfMessageOutcome::AlreadyCancelled,
            CancelTransition::NotCancellable { current } => {
                CancelLxmfMessageOutcome::NotCancellable { current }
            }
        }
    }

    /// Query committed records and overlay only matching active generations as `Sending`.
    pub async fn snapshot(
        &self,
        request: MailboxListRequest,
    ) -> Result<DurableLxmfSnapshot, MailboxFailure> {
        let _transition = self.lifecycle.transition.lock().await;
        let reply = self
            .shared
            .mailbox
            .submit(MailboxRequest::List(request))
            .await;
        let reply = match reply {
            Ok(reply) => {
                self.shared.set_mailbox_ready();
                reply
            }
            Err(failure) => {
                self.shared.set_mailbox_failure(&failure);
                return Err(failure);
            }
        };
        let MailboxReply::Listed {
            mut messages,
            revision,
        } = reply
        else {
            return Err(MailboxFailure::unavailable(
                "the mailbox owner returned an unexpected list reply",
            ));
        };
        let active: Vec<AttemptKey> = self.lifecycle.attempts().keys().copied().collect();
        for message in &mut messages {
            if let Some(key) = active
                .iter()
                .find(|key| key.local_record_id == message.local_record_id)
            {
                let _projected = message.project_sending(key.generation);
            }
        }
        Ok(DurableLxmfSnapshot {
            peers: self.shared.peers.lock().await.values().cloned().collect(),
            messages,
            health: self.shared.health.snapshot(),
            mailbox_health: self.shared.mailbox_health(),
            durable_revision: revision,
            projection_revision: *self.shared.refresh.borrow(),
        })
    }

    /// Stop callback work and attempts without rewriting durable queued rows.
    pub async fn stop(&self) -> Result<(), DirectServiceStopError> {
        if self.lifecycle.stop_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        self.lifecycle.stopped.store(true, Ordering::Release);
        self.shared.health.stop();
        let _changed = self.lifecycle.shutdown.send(true);
        {
            let attempts = self.lifecycle.attempts();
            for attempt in attempts.values() {
                attempt.abort();
            }
        }
        {
            let worker = self.lifecycle.worker();
            if let Some(worker) = worker.as_ref() {
                worker.abort();
            }
        }
        let _transition = tokio::time::timeout(STOP_JOIN_TIMEOUT, self.lifecycle.transition.lock())
            .await
            .map_err(|_| DirectServiceStopError::JoinTimedOut)?;
        if self.lifecycle.stop_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + STOP_JOIN_TIMEOUT;

        let worker = self.lifecycle.worker().take();
        if let Some(mut worker) = worker {
            if tokio::time::timeout_at(deadline, &mut worker)
                .await
                .is_err()
            {
                *self.lifecycle.worker() = Some(worker);
                return Err(DirectServiceStopError::JoinTimedOut);
            }
        }

        let attempts = mem::take(&mut *self.lifecycle.attempts());
        let mut attempts = attempts.into_iter();
        while let Some((key, mut attempt)) = attempts.next() {
            attempt.abort();
            if tokio::time::timeout_at(deadline, &mut attempt)
                .await
                .is_err()
            {
                let mut retained = BTreeMap::new();
                retained.insert(key, attempt);
                retained.extend(attempts);
                *self.lifecycle.attempts() = retained;
                return Err(DirectServiceStopError::JoinTimedOut);
            }
            self.shared.notify();
        }
        self.lifecycle.stop_complete.store(true, Ordering::Release);
        Ok(())
    }
}

impl PendingDurableDirectLxmfService {
    /// Start the prepared service and register every surviving queued generation.
    pub async fn start(
        self,
        network: Arc<dyn DirectNetwork>,
        mailbox: Arc<dyn MailboxSubmitter>,
        clock: Arc<dyn MailboxClock>,
    ) -> Result<DurableDirectLxmfService, DurableDirectServiceStartError> {
        let Self {
            identity,
            callbacks,
            receiver,
            refresh,
            shutdown,
            shutdown_receiver,
        } = self;
        let runtime = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime,
            Err(_) => {
                callbacks.health.stop();
                return Err(DurableDirectServiceStartError::NoTokioRuntime);
            }
        };
        let shared = Arc::new(DurableShared {
            local_destination: identity.destination(),
            identity: Arc::new(identity),
            network,
            mailbox,
            clock,
            peers: Mutex::new(BTreeMap::new()),
            health: callbacks.health.clone(),
            mailbox_health: StdMutex::new(MailboxProjectionHealth::Ready),
            refresh,
        });
        let queued = match shared.mailbox.submit(MailboxRequest::ListQueued).await {
            Ok(queued) => queued,
            Err(failure) => {
                shared.health.stop();
                return Err(DurableDirectServiceStartError::Mailbox(failure));
            }
        };
        let MailboxReply::Listed { messages, .. } = queued else {
            shared.health.stop();
            return Err(DurableDirectServiceStartError::UnexpectedMailboxReply);
        };
        let worker = runtime.spawn(run_durable_worker(
            shared.clone(),
            receiver,
            shutdown_receiver,
        ));
        let lifecycle = Arc::new(DurableLifecycle {
            shutdown,
            stopped: AtomicBool::new(false),
            stop_complete: AtomicBool::new(false),
            transition: Mutex::new(()),
            worker: StdMutex::new(Some(worker)),
            attempts: StdMutex::new(BTreeMap::new()),
        });
        let service = DurableDirectLxmfService {
            shared,
            lifecycle,
            callbacks,
        };
        let _transition = service.lifecycle.transition.lock().await;
        for message in messages {
            spawn_attempt(&service.shared, &service.lifecycle, message);
        }
        drop(_transition);
        Ok(service)
    }
}

impl DurableShared {
    fn notify(&self) {
        self.refresh.send_modify(|revision| {
            *revision = revision.saturating_add(1);
        });
    }

    fn mailbox_health(&self) -> MailboxProjectionHealth {
        self.mailbox_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_mailbox_ready(&self) {
        self.set_mailbox_health(MailboxProjectionHealth::Ready);
    }

    fn set_mailbox_failure(&self, failure: &MailboxFailure) {
        let health = match failure {
            MailboxFailure::Busy => MailboxProjectionHealth::Busy,
            MailboxFailure::Unavailable(detail) => {
                MailboxProjectionHealth::Unavailable(detail.clone())
            }
            MailboxFailure::ResetRequired(reason) => {
                MailboxProjectionHealth::ResetRequired(reason.clone())
            }
        };
        self.set_mailbox_health(health);
    }

    fn set_mailbox_health(&self, next: MailboxProjectionHealth) {
        let mut current = self
            .mailbox_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current != next {
            *current = next;
            drop(current);
            self.notify();
        }
    }
}

fn compose_new_outbound(
    source: [u8; 16],
    destination: [u8; 16],
    timestamp_unix_ms: u64,
    title: &[u8],
    content: &[u8],
    identity: &LocalLxmfIdentity,
) -> Result<NewOutboundMessage, BasicLxmfComposeError> {
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        destination,
        source,
        timestamp_unix_ms,
        title,
        content,
        None,
        identity,
        &mut output,
    )?;
    Ok(NewOutboundMessage {
        message_id: prepared.message_id(),
        source,
        destination,
        timestamp_unix_ms,
        title: title.to_vec(),
        content: content.to_vec(),
        exact_wire: output[..usize::from(prepared.wire_len())].to_vec(),
    })
}

fn spawn_attempt(
    shared: &Arc<DurableShared>,
    lifecycle: &Arc<DurableLifecycle>,
    message: DurableLxmfMessage,
) {
    let Some(generation) = message.generation() else {
        return;
    };
    if !matches!(
        message.delivery_state,
        DurableLxmfDeliveryState::Queued { .. }
    ) {
        return;
    }
    let key = AttemptKey {
        local_record_id: message.local_record_id,
        generation,
    };
    let mut attempts = lifecycle.attempts();
    if attempts.contains_key(&key) {
        return;
    }
    let (registered, registration) = oneshot::channel();
    let task_shared = shared.clone();
    let weak_lifecycle: Weak<DurableLifecycle> = Arc::downgrade(lifecycle);
    let destination = message.destination;
    let exact_wire = message.exact_wire;
    let attempt = tokio::spawn(async move {
        if registration.await.is_err() {
            return;
        }
        let result =
            run_single_attempt(task_shared.network.as_ref(), destination, exact_wire).await;
        settle_durable_attempt(task_shared, weak_lifecycle, key, result).await;
    });
    attempts.insert(key, attempt);
    let _task_alive = registered.send(());
    drop(attempts);
    shared.notify();
}

async fn settle_durable_attempt(
    shared: Arc<DurableShared>,
    lifecycle: Weak<DurableLifecycle>,
    key: AttemptKey,
    result: Result<crate::direct::DirectDeliveryReceipt, DirectSendFailure>,
) {
    let Some(lifecycle) = lifecycle.upgrade() else {
        return;
    };
    let completion = match result {
        Ok(receipt) => AttemptCompletion::Delivered {
            delivered_at_millis: shared.clock.now_unix_millis(),
            rtt_millis: Some(receipt.rtt_millis),
        },
        Err(failure) => AttemptCompletion::Failed { failure },
    };
    let mut shutdown = lifecycle.shutdown.subscribe();
    loop {
        let transition = lifecycle.transition.lock().await;
        if lifecycle.stopped.load(Ordering::Acquire) {
            remove_active_attempt(&shared, &lifecycle, key);
            return;
        }
        let retry = match shared
            .mailbox
            .submit(MailboxRequest::CompleteAttempt { key, completion })
            .await
        {
            Ok(MailboxReply::Completion(CompletionTransition::Applied { .. })) => {
                shared.set_mailbox_ready();
                shared.notify();
                false
            }
            Ok(MailboxReply::Completion(CompletionTransition::Stale { .. })) => {
                shared.set_mailbox_ready();
                false
            }
            Ok(_) => {
                shared.set_mailbox_health(MailboxProjectionHealth::Unavailable(
                    "the mailbox owner returned an unexpected completion reply".to_owned(),
                ));
                true
            }
            Err(failure @ (MailboxFailure::Busy | MailboxFailure::Unavailable(_))) => {
                shared.set_mailbox_failure(&failure);
                true
            }
            Err(failure @ MailboxFailure::ResetRequired(_)) => {
                shared.set_mailbox_failure(&failure);
                false
            }
        };
        if !retry {
            remove_active_attempt(&shared, &lifecycle, key);
            return;
        }
        drop(transition);
        tokio::select! {
            () = tokio::time::sleep(SETTLEMENT_RETRY_DELAY) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    remove_active_attempt(&shared, &lifecycle, key);
                    return;
                }
            }
        }
    }
}

fn remove_active_attempt(shared: &DurableShared, lifecycle: &DurableLifecycle, key: AttemptKey) {
    if lifecycle.attempts().remove(&key).is_some() {
        shared.notify();
    }
}

async fn run_durable_worker(
    shared: Arc<DurableShared>,
    mut jobs: mpsc::Receiver<Job>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            job = jobs.recv() => {
                let Some(job) = job else {
                    shared.health.stop();
                    return;
                };
                process_durable_job(&shared, job).await;
                if jobs.capacity() > 0 {
                    shared.health.recover();
                }
            }
        }
    }
}

async fn process_durable_job(shared: &Arc<DurableShared>, job: Job) {
    match job {
        Job::Peer(job) => {
            let Ok(announce) = parse_lxmf_announce(&job.app_data, MAX_DISPLAY_NAME_BYTES) else {
                return;
            };
            let mut normalized = [0_u8; MAX_DISPLAY_NAME_BYTES];
            let display_name = announce.display_name.and_then(|bytes| {
                normalize_lxmf_display_name(bytes, &mut normalized)
                    .ok()
                    .map(String::from)
            });
            shared.peers.lock().await.insert(
                job.destination,
                LxmfPeer {
                    destination: job.destination,
                    announced_identity: job.announced_identity,
                    display_name,
                    required_stamp_cost: announce.required_stamp_cost,
                    observed_at_millis: job.observed_at_millis,
                    is_path_response: job.is_path_response,
                },
            );
            shared.notify();
        }
        Job::Inbound(job) => process_durable_inbound(shared, job.wire).await,
    }
}

async fn process_durable_inbound(shared: &Arc<DurableShared>, wire: Vec<u8>) {
    let Ok(message) = MessageView::parse_ingress(
        CarrierIngress::LinkDataContextNone {
            expected_destination: &shared.local_destination,
            payload: &wire,
        },
        direct_wire_limits(),
    ) else {
        return;
    };
    if message.payload().stamp().is_some() {
        return;
    }
    let message_id = message.message_id();
    let source = *message.source_hash();
    let verification = match shared.network.destination_public_key(source).await {
        None => LxmfVerification::SourceUnknown,
        Some(public) => match message.bind_source_identity(&public) {
            Ok(bound) if message.verify_signature(&bound).is_ok() => LxmfVerification::Verified,
            Ok(_) | Err(_) => LxmfVerification::InvalidSignature,
        },
    };
    let Some(timestamp_unix_ms) = timestamp_millis(message.payload().timestamp()) else {
        return;
    };
    let inbound = NewInboundMessage {
        message_id,
        source,
        destination: *message.destination_hash(),
        timestamp_unix_ms,
        title: message.payload().title().as_bytes().to_vec(),
        content: message.payload().content().as_bytes().to_vec(),
        exact_wire: wire,
        verification,
    };
    match shared
        .mailbox
        .submit(MailboxRequest::InsertInbound(inbound))
        .await
    {
        Ok(MailboxReply::Inbound(InboundInsertOutcome::Inserted { .. })) => {
            shared.set_mailbox_ready();
            shared.notify();
        }
        Ok(MailboxReply::Inbound(InboundInsertOutcome::Duplicate { .. })) => {
            shared.set_mailbox_ready();
        }
        Ok(_) => shared.set_mailbox_health(MailboxProjectionHealth::Unavailable(
            "the mailbox owner returned an unexpected inbound reply".to_owned(),
        )),
        Err(failure) => shared.set_mailbox_failure(&failure),
    }
}

fn send_failure_outcome(failure: MailboxFailure) -> DurableSendDirectTextOutcome {
    match failure {
        MailboxFailure::Busy => DurableSendDirectTextOutcome::Busy,
        MailboxFailure::Unavailable(detail) => {
            DurableSendDirectTextOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            DurableSendDirectTextOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn retry_failure_outcome(failure: MailboxFailure) -> RetryLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy => RetryLxmfMessageOutcome::Busy,
        MailboxFailure::Unavailable(detail) => {
            RetryLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            RetryLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn cancel_failure_outcome(failure: MailboxFailure) -> CancelLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy => CancelLxmfMessageOutcome::Busy,
        MailboxFailure::Unavailable(detail) => {
            CancelLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            CancelLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    use personal_rns::identity::{IdentityHash, PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::interfaces::InterfaceId;
    use personal_rns::routing::announce::{derive_single_destination_hash, AnnounceObservation};
    use personal_rns::routing::delivery::{Delivery, LinkDelivery};
    use personal_rns::routing::links::LinkId;
    use personal_rns::runtime::{Message, PrnsEvent};
    use personal_rns::units::InstantMillis;
    use personal_rns::wire::DestinationHash;
    use prns_lxmf_wire::{
        compose_basic_direct_lxmf, encode_current_lxmf_announce, BasicLxmfSigner,
    };
    use tokio::sync::Notify;

    use crate::direct::{
        CallbackOutcome, DirectDeliveryReceipt, DirectNetworkFuture, MAX_ANNOUNCE_APP_DATA_BYTES,
    };

    const LOCAL_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x31; IDENTITY_SECRET_KEY_LEN];
    const PEER_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x52; IDENTITY_SECRET_KEY_LEN];
    const TEST_INTERFACE: InterfaceId = InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]);

    struct Signer;

    impl BasicLxmfSigner for Signer {
        fn sign_lxmf(&self, input: &[u8]) -> [u8; 64] {
            let mut signature = [0_u8; 64];
            for (index, byte) in input.iter().enumerate() {
                signature[index % signature.len()] ^= *byte;
            }
            signature
        }
    }

    struct DatabaseSubmitter {
        database: Arc<Database>,
    }

    impl MailboxSubmitter for DatabaseSubmitter {
        fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
            Box::pin(async move { execute_mailbox_request(&self.database, request) })
        }
    }

    struct TransientCompletionSubmitter {
        database: Arc<Database>,
        remaining_failures: AtomicUsize,
        completion_submissions: AtomicUsize,
    }

    impl MailboxSubmitter for TransientCompletionSubmitter {
        fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
            Box::pin(async move {
                if matches!(&request, MailboxRequest::CompleteAttempt { .. }) {
                    self.completion_submissions.fetch_add(1, Ordering::AcqRel);
                    if self
                        .remaining_failures
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                            remaining.checked_sub(1)
                        })
                        .is_ok()
                    {
                        return Err(MailboxFailure::Busy);
                    }
                }
                execute_mailbox_request(&self.database, request)
            })
        }
    }

    struct AlwaysBusySubmitter;

    impl MailboxSubmitter for AlwaysBusySubmitter {
        fn submit(&self, _request: MailboxRequest) -> MailboxFuture<'_> {
            Box::pin(async { Err(MailboxFailure::Busy) })
        }
    }

    struct FixedClock(u64);

    impl MailboxClock for FixedClock {
        fn now_unix_millis(&self) -> u64 {
            self.0
        }
    }

    struct DurableFakeNetwork {
        has_route: AtomicBool,
        results: StdMutex<VecDeque<Result<DirectDeliveryReceipt, DirectSendFailure>>>,
        public_keys: StdMutex<BTreeMap<[u8; 16], [u8; 64]>>,
        sent_wires: StdMutex<Vec<Vec<u8>>>,
        block_send: AtomicBool,
        send_entered: Notify,
        send_released: Notify,
        send_future_dropped: AtomicBool,
    }

    struct DropSignal<'a>(&'a AtomicBool);

    impl Drop for DropSignal<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl Default for DurableFakeNetwork {
        fn default() -> Self {
            Self {
                has_route: AtomicBool::new(true),
                results: StdMutex::new(VecDeque::from([Ok(DirectDeliveryReceipt {
                    rtt_millis: 23,
                })])),
                public_keys: StdMutex::new(BTreeMap::new()),
                sent_wires: StdMutex::new(Vec::new()),
                block_send: AtomicBool::new(false),
                send_entered: Notify::new(),
                send_released: Notify::new(),
                send_future_dropped: AtomicBool::new(false),
            }
        }
    }

    impl DurableFakeNetwork {
        fn release_send(&self) {
            self.block_send.store(false, Ordering::Release);
            self.send_released.notify_waiters();
        }

        fn add_public_key(&self, destination: [u8; 16], material: &PrivateIdentityMaterial) {
            self.public_keys
                .lock()
                .unwrap()
                .insert(destination, *material.public().as_bytes());
        }
    }

    impl DirectNetwork for DurableFakeNetwork {
        fn has_route(&self, _destination: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
            Box::pin(async move { self.has_route.load(Ordering::Acquire) })
        }

        fn request_path(
            &self,
            _destination: [u8; 16],
        ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
            Box::pin(async move { Ok(()) })
        }

        fn establish_link(
            &self,
            _destination: [u8; 16],
        ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
            Box::pin(async move { Ok([0xa5; 16]) })
        }

        fn send_link_packet(
            &self,
            _link: [u8; 16],
            complete_wire: Vec<u8>,
        ) -> DirectNetworkFuture<'_, Result<DirectDeliveryReceipt, DirectSendFailure>> {
            Box::pin(async move {
                let _drop_signal = DropSignal(&self.send_future_dropped);
                self.sent_wires.lock().unwrap().push(complete_wire);
                self.send_entered.notify_one();
                while self.block_send.load(Ordering::Acquire) {
                    self.send_released.notified().await;
                }
                self.results
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(Ok(DirectDeliveryReceipt { rtt_millis: 23 }))
            })
        }

        fn destination_public_key(
            &self,
            destination: [u8; 16],
        ) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
            Box::pin(async move { self.public_keys.lock().unwrap().get(&destination).copied() })
        }

        fn announce(
            &self,
            _destination: [u8; 16],
        ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
            Box::pin(async move { Ok(()) })
        }
    }

    fn open_database() -> (tempfile::TempDir, Database) {
        let root = tempfile::tempdir().unwrap();
        let database = Database::create(root.path().join("application.redb")).unwrap();
        let write = database.begin_write().unwrap();
        write.open_table(DEVELOPMENT_META).unwrap();
        initialize_mailbox_tables(&write).unwrap();
        write.commit().unwrap();
        (root, database)
    }

    fn shared_database() -> (tempfile::TempDir, Arc<Database>, Arc<DatabaseSubmitter>) {
        let (root, database) = open_database();
        let database = Arc::new(database);
        let submitter = Arc::new(DatabaseSubmitter {
            database: database.clone(),
        });
        (root, database, submitter)
    }

    fn peer_facts() -> (PrivateIdentityMaterial, [u8; 16]) {
        let material = PrivateIdentityMaterial::from_bytes(PEER_SECRET);
        let destination = derive_single_destination_hash(
            &material.identity_hash(),
            prns_lxmf_wire::LXMF_APP_NAME,
            prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
        )
        .unwrap();
        (material, *destination.as_bytes())
    }

    fn accepted_observation<'a>(
        destination: [u8; 16],
        identity: IdentityHash,
        app_data: &'a [u8],
    ) -> AnnounceObservation<'a> {
        AnnounceObservation {
            destination: DestinationHash::new(destination),
            announced_identity: identity,
            hops: personal_rns::units::HopCount(1),
            source_interface: TEST_INTERFACE,
            arrived_at: InstantMillis(100),
            app_data,
            is_path_response: false,
        }
    }

    fn current_announce() -> Vec<u8> {
        let mut output = [0_u8; MAX_ANNOUNCE_APP_DATA_BYTES];
        let len = encode_current_lxmf_announce(b"peer", &mut output).unwrap();
        output[..len].to_vec()
    }

    fn link_event(wire: &[u8]) -> PrnsEvent<'_> {
        PrnsEvent::Message(Message::Delivered(Delivery::Link(LinkDelivery {
            link_id: LinkId::new([0xc7; 16]),
            plaintext: wire,
            arrived_at: InstantMillis(200),
            source_interface: TEST_INTERFACE,
        })))
    }

    fn all_messages() -> MailboxListRequest {
        MailboxListRequest {
            peer: None,
            direction: None,
            before: None,
            limit: 100,
        }
    }

    async fn wait_for_durable_snapshot(
        service: &DurableDirectLxmfService,
        predicate: impl Fn(&DurableLxmfSnapshot) -> bool,
    ) -> DurableLxmfSnapshot {
        for _ in 0..100 {
            let snapshot = service.snapshot(all_messages()).await.unwrap();
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("durable snapshot predicate did not settle");
    }

    async fn start_durable(
        network: Arc<DurableFakeNetwork>,
        submitter: Arc<DatabaseSubmitter>,
    ) -> DurableDirectLxmfService {
        let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
        let (pending, callbacks) = DurableDirectLxmfService::prepare(identity);
        let service = pending
            .start(network, submitter, Arc::new(FixedClock(9_999)))
            .await
            .unwrap();
        assert_eq!(
            service.callbacks().local_destination,
            callbacks.local_destination
        );
        service
    }

    async fn learn_durable_peer(service: &DurableDirectLxmfService) -> [u8; 16] {
        let (peer, destination) = peer_facts();
        let announce = current_announce();
        assert_eq!(
            service
                .callbacks()
                .on_accepted_announce(accepted_observation(
                    destination,
                    peer.identity_hash(),
                    &announce,
                )),
            CallbackOutcome::Enqueued
        );
        wait_for_durable_snapshot(service, |snapshot| !snapshot.peers.is_empty()).await;
        destination
    }

    fn outbound(timestamp: u64) -> NewOutboundMessage {
        let source = [0x11; 16];
        let destination = [0x22; 16];
        let title = b"title".to_vec();
        let content = b"content".to_vec();
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            destination,
            source,
            timestamp,
            &title,
            &content,
            None,
            &Signer,
            &mut output,
        )
        .unwrap();
        NewOutboundMessage {
            message_id: prepared.message_id(),
            source,
            destination,
            timestamp_unix_ms: timestamp,
            title,
            content,
            exact_wire: output[..usize::from(prepared.wire_len())].to_vec(),
        }
    }

    fn inserted_outbound(database: &Database, timestamp: u64) -> DurableLxmfMessage {
        match execute_mailbox_request(
            database,
            MailboxRequest::InsertOutbound(outbound(timestamp)),
        )
        .unwrap()
        {
            MailboxReply::OutboundInserted { message, .. } => message,
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn accepted_wire_reopens_as_queued_and_validates_exactly() {
        let (root, database) = open_database();
        let message = inserted_outbound(&database, 1_700_000_000_123);
        assert_eq!(message.local_record_id, 1);
        assert_eq!(message.generation(), Some(1));
        assert_eq!(
            message.delivery_state,
            DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
        );
        let exact_wire = message.exact_wire.clone();
        drop(database);

        let reopened = Database::open(root.path().join("application.redb")).unwrap();
        validate_mailbox_database(&reopened).unwrap();
        let listed = execute_mailbox_request(
            &reopened,
            MailboxRequest::List(MailboxListRequest {
                peer: None,
                direction: None,
                before: None,
                limit: 10,
            }),
        )
        .unwrap();
        let MailboxReply::Listed { messages, revision } = listed else {
            panic!("unexpected list reply");
        };
        assert_eq!(revision, 1);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].exact_wire, exact_wire);
    }

    #[test]
    fn retry_preserves_wire_and_message_id_and_rejects_old_generation() {
        let (_root, database) = open_database();
        let original = inserted_outbound(&database, 1_700_000_000_456);
        let first_key = AttemptKey {
            local_record_id: original.local_record_id,
            generation: original.generation().unwrap(),
        };
        let failed = execute_mailbox_request(
            &database,
            MailboxRequest::CompleteAttempt {
                key: first_key,
                completion: AttemptCompletion::Failed {
                    failure: DirectSendFailure::DeliveryTimedOut,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            failed,
            MailboxReply::Completion(CompletionTransition::Applied {
                message: DurableLxmfMessage {
                    delivery_state: DurableLxmfDeliveryState::Failed {
                        failed_attempts: 1,
                        last_failure: DirectSendFailure::DeliveryTimedOut,
                    },
                    ..
                },
                ..
            })
        ));

        let retried = execute_mailbox_request(
            &database,
            MailboxRequest::Retry {
                local_record_id: original.local_record_id,
            },
        )
        .unwrap();
        let MailboxReply::Retry(RetryTransition::Accepted { message, .. }) = retried else {
            panic!("retry was not accepted");
        };
        assert_eq!(message.generation(), Some(2));
        assert_eq!(message.message_id, original.message_id);
        assert_eq!(message.exact_wire, original.exact_wire);
        assert_eq!(
            message.delivery_state,
            DurableLxmfDeliveryState::Queued { failed_attempts: 1 }
        );

        let late = execute_mailbox_request(
            &database,
            MailboxRequest::CompleteAttempt {
                key: first_key,
                completion: AttemptCompletion::Delivered {
                    delivered_at_millis: 99,
                    rtt_millis: Some(7),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            late,
            MailboxReply::Completion(CompletionTransition::Stale {
                current: Some(DurableLxmfDeliveryState::Queued { failed_attempts: 1 })
            })
        ));
    }

    #[test]
    fn proof_and_cancel_are_linearized_by_the_first_transaction() {
        let (_root, database) = open_database();
        let delivered = inserted_outbound(&database, 1_700_000_001_000);
        let key = AttemptKey {
            local_record_id: delivered.local_record_id,
            generation: delivered.generation().unwrap(),
        };
        execute_mailbox_request(
            &database,
            MailboxRequest::CompleteAttempt {
                key,
                completion: AttemptCompletion::Delivered {
                    delivered_at_millis: 10,
                    rtt_millis: Some(3),
                },
            },
        )
        .unwrap();
        assert_eq!(
            execute_mailbox_request(
                &database,
                MailboxRequest::Cancel {
                    local_record_id: delivered.local_record_id,
                    cancelled_at_millis: 11,
                },
            )
            .unwrap(),
            MailboxReply::Cancel(CancelTransition::AlreadyDelivered)
        );

        let cancelled = inserted_outbound(&database, 1_700_000_002_000);
        let cancelled_key = AttemptKey {
            local_record_id: cancelled.local_record_id,
            generation: cancelled.generation().unwrap(),
        };
        assert!(matches!(
            execute_mailbox_request(
                &database,
                MailboxRequest::Cancel {
                    local_record_id: cancelled.local_record_id,
                    cancelled_at_millis: 12,
                },
            )
            .unwrap(),
            MailboxReply::Cancel(CancelTransition::Cancelled { .. })
        ));
        assert!(matches!(
            execute_mailbox_request(
                &database,
                MailboxRequest::CompleteAttempt {
                    key: cancelled_key,
                    completion: AttemptCompletion::Delivered {
                        delivered_at_millis: 13,
                        rtt_millis: None,
                    },
                },
            )
            .unwrap(),
            MailboxReply::Completion(CompletionTransition::Stale {
                current: Some(DurableLxmfDeliveryState::Cancelled {
                    cancelled_at_millis: 12
                })
            })
        ));
    }

    #[test]
    fn duplicate_inbound_is_noop_and_does_not_advance_revision() {
        let (_root, database) = open_database();
        let outbound = outbound(1_700_000_003_000);
        let inbound = NewInboundMessage {
            message_id: outbound.message_id,
            source: outbound.source,
            destination: outbound.destination,
            timestamp_unix_ms: outbound.timestamp_unix_ms,
            title: outbound.title,
            content: outbound.content,
            exact_wire: outbound.exact_wire,
            verification: LxmfVerification::SourceUnknown,
        };
        let first =
            execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound.clone()))
                .unwrap();
        assert!(matches!(
            first,
            MailboxReply::Inbound(InboundInsertOutcome::Inserted { revision: 1, .. })
        ));
        let duplicate =
            execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound)).unwrap();
        assert_eq!(
            duplicate,
            MailboxReply::Inbound(InboundInsertOutcome::Duplicate { local_record_id: 1 })
        );
        let listed = execute_mailbox_request(
            &database,
            MailboxRequest::List(MailboxListRequest {
                peer: None,
                direction: None,
                before: None,
                limit: 10,
            }),
        )
        .unwrap();
        assert!(matches!(
            listed,
            MailboxReply::Listed {
                revision: 1,
                ref messages
            } if messages.len() == 1
        ));
        validate_mailbox_database(&database).unwrap();
    }

    #[test]
    fn validation_rejects_missing_index_and_exact_wire_drift() {
        let (_root, database) = open_database();
        let outbound = outbound(1_700_000_004_000);
        let inbound = NewInboundMessage {
            message_id: outbound.message_id,
            source: outbound.source,
            destination: outbound.destination,
            timestamp_unix_ms: outbound.timestamp_unix_ms,
            title: outbound.title,
            content: outbound.content,
            exact_wire: outbound.exact_wire,
            verification: LxmfVerification::Verified,
        };
        execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound)).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut index = write.open_table(LXMF_INBOUND_IDS).unwrap();
            index.remove([0x22; 32].as_slice()).unwrap();
            let first = index
                .iter()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .0
                .value()
                .to_vec();
            index.remove(first.as_slice()).unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));

        let (_root, database) = open_database();
        let message = inserted_outbound(&database, 1_700_000_005_000);
        let write = database.begin_write().unwrap();
        {
            let mut table = write.open_table(LXMF_MESSAGES).unwrap();
            let encoded = table
                .get(message.local_record_id)
                .unwrap()
                .unwrap()
                .value()
                .to_vec();
            let mut record: StoredRecord = serde_json::from_slice(&encoded).unwrap();
            record.content.push(0xff);
            let encoded = serde_json::to_vec(&record).unwrap();
            table
                .insert(message.local_record_id, encoded.as_slice())
                .unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));
    }

    #[test]
    fn validation_requires_the_nullable_receipt_field() {
        let (_root, database) = open_database();
        let message = inserted_outbound(&database, 1_700_000_005_500);
        let key = AttemptKey {
            local_record_id: message.local_record_id,
            generation: message.generation().unwrap(),
        };
        execute_mailbox_request(
            &database,
            MailboxRequest::CompleteAttempt {
                key,
                completion: AttemptCompletion::Delivered {
                    delivered_at_millis: 10,
                    rtt_millis: None,
                },
            },
        )
        .unwrap();

        let write = database.begin_write().unwrap();
        {
            let mut table = write.open_table(LXMF_MESSAGES).unwrap();
            let encoded = table
                .get(message.local_record_id)
                .unwrap()
                .unwrap()
                .value()
                .to_vec();
            let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
            value["recordKind"]["state"]
                .as_object_mut()
                .unwrap()
                .remove("rttMillis");
            let encoded = serde_json::to_vec(&value).unwrap();
            table
                .insert(message.local_record_id, encoded.as_slice())
                .unwrap();
        }
        write.commit().unwrap();

        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));
    }

    #[test]
    fn validation_rejects_impossible_counter_id_and_generation_history() {
        let (_root, database) = open_database();
        inserted_outbound(&database, 1_700_000_005_600);
        let write = database.begin_write().unwrap();
        {
            let mut meta = write.open_table(DEVELOPMENT_META).unwrap();
            meta.insert(LXMF_REVISION_KEY, 0).unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));

        let (_root, database) = open_database();
        inserted_outbound(&database, 1_700_000_005_700);
        inserted_outbound(&database, 1_700_000_005_800);
        let write = database.begin_write().unwrap();
        {
            let mut messages = write.open_table(LXMF_MESSAGES).unwrap();
            messages.remove(1).unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));

        let (_root, database) = open_database();
        let message = inserted_outbound(&database, 1_700_000_005_900);
        let write = database.begin_write().unwrap();
        {
            let mut messages = write.open_table(LXMF_MESSAGES).unwrap();
            let encoded = messages
                .get(message.local_record_id)
                .unwrap()
                .unwrap()
                .value()
                .to_vec();
            let mut record: StoredRecord = serde_json::from_slice(&encoded).unwrap();
            let StoredRecordKind::Outbound { generation, .. } = &mut record.record_kind else {
                panic!("inserted outbound record decoded as inbound");
            };
            *generation = 9;
            let encoded = serde_json::to_vec(&record).unwrap();
            messages
                .insert(message.local_record_id, encoded.as_slice())
                .unwrap();
        }
        write.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&database),
            Err(MailboxFailure::ResetRequired(_))
        ));
    }

    #[tokio::test]
    async fn durable_send_accepts_before_proof_and_projects_receipt_settlement() {
        let (_root, _database, submitter) = shared_database();
        let network = Arc::new(DurableFakeNetwork::default());
        network.block_send.store(true, Ordering::Release);
        let service = start_durable(network.clone(), submitter).await;
        let destination = learn_durable_peer(&service).await;
        let before = *service.subscribe().borrow();

        assert_eq!(
            service
                .send_direct_text(
                    destination,
                    1_700_000_006_000,
                    b"durable",
                    b"accepted before proof",
                )
                .await,
            DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
        );
        network.send_entered.notified().await;
        let sending = service.snapshot(all_messages()).await.unwrap();
        assert!(matches!(
            sending.messages[0].delivery_state,
            DurableLxmfDeliveryState::Sending { failed_attempts: 0 }
        ));
        assert_eq!(sending.durable_revision, 1);
        assert_eq!(sending.projection_revision, before + 2);

        network.release_send();
        let delivered = wait_for_durable_snapshot(&service, |snapshot| {
            matches!(
                snapshot.messages[0].delivery_state,
                DurableLxmfDeliveryState::Delivered {
                    delivered_at_millis: 9_999,
                    rtt_millis: Some(23),
                }
            )
        })
        .await;
        assert_eq!(delivered.durable_revision, 2);
        assert_eq!(delivered.projection_revision, before + 4);
        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn transient_settlement_admission_failure_does_not_strand_a_queued_row() {
        let (_root, database) = open_database();
        let database = Arc::new(database);
        let submitter = Arc::new(TransientCompletionSubmitter {
            database,
            remaining_failures: AtomicUsize::new(1),
            completion_submissions: AtomicUsize::new(0),
        });
        let network = Arc::new(DurableFakeNetwork::default());
        let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
        let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
        let service = pending
            .start(
                network.clone(),
                submitter.clone(),
                Arc::new(FixedClock(9_999)),
            )
            .await
            .unwrap();
        let destination = learn_durable_peer(&service).await;

        assert_eq!(
            service
                .send_direct_text(
                    destination,
                    1_700_000_006_500,
                    b"settle",
                    b"retry commit only",
                )
                .await,
            DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
        );
        let delivered = wait_for_durable_snapshot(&service, |snapshot| {
            matches!(
                snapshot.messages[0].delivery_state,
                DurableLxmfDeliveryState::Delivered { .. }
            )
        })
        .await;

        assert_eq!(delivered.durable_revision, 2);
        assert!(submitter.completion_submissions.load(Ordering::Acquire) >= 2);
        assert_eq!(network.sent_wires.lock().unwrap().len(), 1);
        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_stop_waiters_do_not_report_before_the_join_boundary() {
        let (_root, _database, submitter) = shared_database();
        let service = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
        let transition = service.lifecycle.transition.lock().await;
        let first_service = service.clone();
        let first = tokio::spawn(async move { first_service.stop().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let second_service = service.clone();
        let second = tokio::spawn(async move { second_service.stop().await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(!first.is_finished());
        assert!(!second.is_finished());
        drop(transition);
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));

        let transition = service.lifecycle.transition.lock().await;
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(10), service.stop())
                .await
                .unwrap(),
            Ok(())
        );
        drop(transition);
    }

    #[tokio::test]
    async fn start_failure_stops_and_joins_the_prepared_callback_worker() {
        let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
        let (pending, callbacks) = DurableDirectLxmfService::prepare(identity);
        let result = pending
            .start(
                Arc::new(DurableFakeNetwork::default()),
                Arc::new(AlwaysBusySubmitter),
                Arc::new(FixedClock(9_999)),
            )
            .await;

        assert!(matches!(
            result,
            Err(DurableDirectServiceStartError::Mailbox(
                MailboxFailure::Busy
            ))
        ));
        assert_eq!(callbacks.health.state(), crate::LxmfHealthState::Stopped);
    }

    #[tokio::test]
    async fn stop_leaves_queued_and_restart_resumes_the_identical_wire() {
        let (_root, database, submitter) = shared_database();
        let first_network = Arc::new(DurableFakeNetwork::default());
        first_network.block_send.store(true, Ordering::Release);
        let first = start_durable(first_network.clone(), submitter.clone()).await;
        let destination = learn_durable_peer(&first).await;
        assert!(matches!(
            first
                .send_direct_text(destination, 1_700_000_007_000, b"restart", b"same wire")
                .await,
            DurableSendDirectTextOutcome::Accepted { .. }
        ));
        first_network.send_entered.notified().await;
        let first_wire = first_network.sent_wires.lock().unwrap()[0].clone();
        first.stop().await.unwrap();
        assert!(first_network.send_future_dropped.load(Ordering::Acquire));

        let MailboxReply::Listed { messages, .. } =
            execute_mailbox_request(&database, MailboxRequest::List(all_messages())).unwrap()
        else {
            panic!("unexpected list reply");
        };
        assert!(matches!(
            messages[0].delivery_state,
            DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
        ));

        let second_network = Arc::new(DurableFakeNetwork::default());
        let second = start_durable(second_network.clone(), submitter).await;
        let delivered = wait_for_durable_snapshot(&second, |snapshot| {
            matches!(
                snapshot.messages[0].delivery_state,
                DurableLxmfDeliveryState::Delivered { .. }
            )
        })
        .await;
        assert_eq!(delivered.messages[0].exact_wire, first_wire);
        assert_eq!(second_network.sent_wires.lock().unwrap()[0], first_wire);
        second.stop().await.unwrap();
    }

    #[tokio::test]
    async fn durable_retry_is_byte_identical_and_cancel_wins_before_late_proof() {
        let (_root, _database, submitter) = shared_database();
        let network = Arc::new(DurableFakeNetwork::default());
        *network.results.lock().unwrap() = VecDeque::from([
            Err(DirectSendFailure::DeliveryTimedOut),
            Ok(DirectDeliveryReceipt { rtt_millis: 31 }),
        ]);
        let service = start_durable(network.clone(), submitter).await;
        let destination = learn_durable_peer(&service).await;
        assert_eq!(
            service
                .send_direct_text(destination, 1_700_000_008_000, b"retry", b"same")
                .await,
            DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
        );
        wait_for_durable_snapshot(&service, |snapshot| {
            matches!(
                snapshot.messages[0].delivery_state,
                DurableLxmfDeliveryState::Failed {
                    failed_attempts: 1,
                    last_failure: DirectSendFailure::DeliveryTimedOut,
                }
            )
        })
        .await;
        let first_wire = network.sent_wires.lock().unwrap()[0].clone();

        network.block_send.store(true, Ordering::Release);
        assert_eq!(
            service.retry_lxmf_message(1).await,
            RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
        );
        while network.sent_wires.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(network.sent_wires.lock().unwrap()[1], first_wire);
        assert_eq!(
            service.cancel_lxmf_message(1, 12_345).await,
            CancelLxmfMessageOutcome::Cancelled { local_record_id: 1 }
        );
        network.release_send();
        let cancelled = service.snapshot(all_messages()).await.unwrap();
        assert!(matches!(
            cancelled.messages[0].delivery_state,
            DurableLxmfDeliveryState::Cancelled {
                cancelled_at_millis: 12_345
            }
        ));
        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn proof_first_inbound_commit_deduplicates_across_restart() {
        let (_root, _database, submitter) = shared_database();
        let network = Arc::new(DurableFakeNetwork::default());
        let (peer, peer_destination) = peer_facts();
        network.add_public_key(peer_destination, &peer);
        let service = start_durable(network, submitter.clone()).await;
        let local_destination = service.local_destination();
        let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            local_destination,
            peer_destination,
            1_700_000_009_000,
            b"inbound",
            b"persisted",
            None,
            &signer,
            &mut output,
        )
        .unwrap();
        let wire = output[..usize::from(prepared.wire_len())].to_vec();
        assert_eq!(
            service.callbacks().on_prns_event(link_event(&wire)),
            CallbackOutcome::Enqueued
        );
        let first =
            wait_for_durable_snapshot(&service, |snapshot| snapshot.messages.len() == 1).await;
        assert_eq!(first.durable_revision, 1);
        assert_eq!(first.messages[0].verification, LxmfVerification::Verified);

        assert_eq!(
            service.callbacks().on_prns_event(link_event(&wire)),
            CallbackOutcome::Enqueued
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        let duplicate = service.snapshot(all_messages()).await.unwrap();
        assert_eq!(duplicate.messages.len(), 1);
        assert_eq!(duplicate.durable_revision, 1);
        service.stop().await.unwrap();

        let restarted = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
        let reopened = restarted.snapshot(all_messages()).await.unwrap();
        assert_eq!(reopened.messages.len(), 1);
        assert_eq!(reopened.messages[0].exact_wire, wire);
        restarted.stop().await.unwrap();
    }
}
