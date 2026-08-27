use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::engine::{
    CommandId, Departure, IssuedCommand, PersistenceFlushCause, PersistenceFlushTarget,
    SendRequestFailure, Settlement,
};
use crate::interfaces::{ConnectionView, InterfaceDescriptor, InterfaceId};
use crate::manifold::grant_lane::{TokioGrantConsumer, TokioGrantProducer};
use crate::routing::links::channel::byte_stream::StreamId;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceHash, ResourceMetadata, ResourceStrategy};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::runtime::node_introspection::NodeIntrospectionRequest;
#[cfg(feature = "runtime-metrics")]
use crate::runtime::RuntimeMetricsSnapshot;
use crate::runtime::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionHostCommand, DropRouteOutcome,
    DropRoutesViaOutcome, IdentityBlackholeHostCommand,
};
use crate::storage::TablePushError;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, TransportId};
use prns_runtime::runtime::{PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot};

#[allow(clippy::large_enum_variant)]
pub enum HostCommand {
    Engine(IssuedCommand),
    NotePersistenceFlush {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
        observed: Option<oneshot::Sender<()>>,
    },
    NotePersistenceFlushFailure {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
        observed: oneshot::Sender<()>,
    },
    AwaitedEngine {
        issued: IssuedCommand,
        completion: oneshot::Sender<Settlement>,
    },
    SendResource(SendResourceHostCommand),
    SendResourceSegment(SendResourceSegmentHostCommand),
    RespondAny(RespondAnyHostCommand),
    RequestAny(RequestAnyHostCommand),
    ProvideDecompressed(ProvideDecompressedHostCommand),
    AddInterface(AddInterfaceCommand),
    RemoveInterface {
        id: InterfaceId,
        departure: Departure,
    },
    DropRoute {
        destination: DestinationHash,
        reply: oneshot::Sender<DropRouteOutcome>,
    },
    DropRoutesVia {
        transport: TransportId,
        reply: oneshot::Sender<DropRoutesViaOutcome>,
    },
    ClearAnnounceQueues {
        reply: oneshot::Sender<ClearAnnounceQueuesOutcome>,
    },
    IdentityBlackhole(IdentityBlackholeHostCommand),
    DestinationIdentityRetention(DestinationIdentityRetentionHostCommand),
    NodeIntrospection(NodeIntrospectionRequest),
    SynthesizeTunnel {
        interface: InterfaceId,
    },
    /// Register a byte-stream reader's inbound sink: the run loop routes matching channel messages to it, suppressed from the app event stream. `ready` fires once the sink is in the routing table, so the opener can hold back the reader until no arriving chunk can slip past; awaited, not raced.
    RegisterStreamReader {
        link_id: LinkId,
        stream_id: StreamId,
        sink: UnboundedSender<StreamInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Register a sink for the next inbound resource on this link: the run loop routes the resource's chunks to it and signals completion, suppressed from the app event stream. `ready` fires once registered, so a segment arriving the instant after cannot slip past to the app.
    RegisterResourceSink {
        link_id: LinkId,
        sink: UnboundedSender<ResourceInbound>,
        ready: oneshot::Sender<()>,
    },
    /// Set the default resource strategy for `destination`. `ready` carries back whether the destination was held, so a caller learns a misaddressed strategy rather than having it silently dropped.
    SetResourceStrategy {
        destination: DestinationHash,
        strategy: ResourceStrategy,
        ready: oneshot::Sender<bool>,
    },
    /// Mutate a request route on the manifold and acknowledge the exact table
    /// outcome before the host publishes matching application state.
    RegisterRequestHandler {
        destination: DestinationHash,
        path_hash: RequestPathHash,
        policy: RequestPolicy,
        ready: oneshot::Sender<Result<(), TablePushError>>,
    },
    /// Remove a request route on the manifold. The boolean distinguishes a
    /// landed removal from an already-absent, idempotent reconciliation.
    UnregisterRequestHandler {
        destination: DestinationHash,
        path_hash: RequestPathHash,
        ready: oneshot::Sender<bool>,
    },
    /// Serialize every persisted region on the manifold — the one place a consistent view exists — and hand the sealed images back; the caller owns the store IO, so flush cadence stays host policy.
    SnapshotPersistedState {
        reply: oneshot::Sender<PersistedStateSnapshot>,
    },
    /// Seal every tracked destination's self-ratchet record. Secrets ride these blobs, so they go to the caller's identity vault, never a `PersistedStore`.
    SnapshotSelfRatchets {
        reply: oneshot::Sender<SelfRatchetsSnapshot>,
    },
    SnapshotSelfRatchet {
        destination: DestinationHash,
        reply: oneshot::Sender<Option<SelfRatchetSnapshot>>,
    },
    #[cfg(feature = "runtime-metrics")]
    SnapshotMetrics {
        reply: oneshot::Sender<RuntimeMetricsSnapshot>,
    },
}

pub struct StreamInbound {
    pub payload: std::vec::Vec<u8>,
    pub eof: bool,
    pub compressed: bool,
}

pub enum ResourceInbound {
    /// The transfer's packed metadata, arriving ahead of the first chunk when one traveled.
    Metadata(std::vec::Vec<u8>),
    Chunk(std::vec::Vec<u8>),
    Complete {
        original_hash: ResourceHash,
        total_size_bytes: u64,
    },
    Failed,
}

pub struct AddInterfaceCommand {
    pub descriptor: InterfaceDescriptor,
    pub logical_interface: InterfaceId,
    pub inbound: TokioGrantConsumer,
    pub egress: TokioGrantProducer,
    pub connection: Option<ConnectionView>,
    pub frame_accounting: Option<crate::interfaces::FrameAccountingRecorder>,
    pub ifac: Option<crate::interfaces::IfacContext>,
}

#[derive(Debug)]
enum HostResourceStorage {
    Owned(std::vec::Vec<u8>),
    Shared(Arc<[u8]>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostResourcePayloadError {
    PrefixOutOfRange,
}

#[derive(Debug)]
pub struct HostResourcePayload {
    storage: HostResourceStorage,
    len: usize,
}

impl HostResourcePayload {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        match &self.storage {
            HostResourceStorage::Owned(bytes) => &bytes[..self.len],
            HostResourceStorage::Shared(bytes) => &bytes[..self.len],
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn shared_prefix(bytes: Arc<[u8]>, len: usize) -> Result<Self, HostResourcePayloadError> {
        if len > bytes.len() {
            return Err(HostResourcePayloadError::PrefixOutOfRange);
        }
        Ok(Self {
            storage: HostResourceStorage::Shared(bytes),
            len,
        })
    }
}

impl AsRef<[u8]> for HostResourcePayload {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<std::vec::Vec<u8>> for HostResourcePayload {
    fn from(bytes: std::vec::Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            storage: HostResourceStorage::Owned(bytes),
            len,
        }
    }
}

impl From<Arc<[u8]>> for HostResourcePayload {
    fn from(bytes: Arc<[u8]>) -> Self {
        let len = bytes.len();
        Self {
            storage: HostResourceStorage::Shared(bytes),
            len,
        }
    }
}

/// The host half of [`ResourceMetadata`]: owned packed bytes crossing to the node thread.
pub enum HostResourceMetadata {
    None,
    /// This (first or only) segment carries the block in-stream.
    Packed(HostResourcePayload),
    /// A later segment of a split whose first segment carried the block.
    SentInFirstSegment {
        packed_len: u32,
    },
}

impl HostResourceMetadata {
    pub(super) fn as_engine(&self) -> ResourceMetadata<'_> {
        match self {
            Self::None => ResourceMetadata::None,
            Self::Packed(payload) => ResourceMetadata::Packed(payload.as_slice()),
            Self::SentInFirstSegment { packed_len } => ResourceMetadata::SentInFirstSegment {
                packed_len: *packed_len,
            },
        }
    }
}

pub struct SendResourceHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
    pub metadata: HostResourceMetadata,
    pub request_id: Option<RequestId>,
}

/// One segment of a resource send, awaited: the `completion` rides the command to the manifold, which stashes it keyed by `id` and fires it when the segment's proof settles — so a host `send_resource` loop drains its source one segment at a time, sending the next only once the last is proven. The engine threads the chain's original hash across segments; the host carries only the bytes.
pub struct SendResourceSegmentHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub data: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
    pub metadata: HostResourceMetadata,
    pub request_id: Option<RequestId>,
    pub segment_index: u64,
    pub total_segments: u64,
    pub total_data_bytes: u64,
    pub completion: oneshot::Sender<Settlement>,
}

/// Answer a request with `data` of any length: the engine picks the rung, a single RESPONSE packet when it fits the link MDU, an outgoing resource (named back to `request_id`) when it doesn't. Host-held payload, since a large answer never rides an enum; the request router's verb.
pub struct RespondAnyHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub packed: HostResourcePayload,
    pub compressed_candidate: Option<HostResourcePayload>,
    /// Present when the caller must not issue another response Resource on this
    /// link until the packet write or Resource proof has settled.
    pub completion: Option<oneshot::Sender<Settlement>>,
}

/// Make a request of `path_hash` with `data` of any length: the manifold picks the rung and fires `completion` with the response bytes and the round trip once the answer settles. The payload is host-held like every owned-bytes verb.
pub struct RequestAnyHostCommand {
    pub id: CommandId,
    pub link_id: LinkId,
    pub path_hash: RequestPathHash,
    pub data: HostResourcePayload,
    pub response_timeout: crate::engine::RequestResponseTimeout,
    pub maximum_response_bytes: crate::units::ByteLimit,
    pub completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
}

pub struct ProvideDecompressedHostCommand {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub plaintext: HostResourcePayload,
}

#[cfg(test)]
mod tests;
