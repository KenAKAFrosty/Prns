use std::collections::VecDeque;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::engine::{RespondFailure, SendResourceFailure, Settlement};
use crate::manifold::compression;
use crate::manifold::driver::{
    HostCommand, HostResourceMetadata, HostResourcePayload, ResourceInbound,
    SendResourceSegmentHostCommand,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{
    sealed_transfer_len, ResourceHash, ResourceStrategy, MAX_EFFICIENT_SIZE, METADATA_PREFIX_LEN,
};
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::PrnsNodeHandle;

#[derive(Debug)]
pub enum ResourceSendError {
    Source(std::io::Error),
    UnrepresentableLength,
    Rejected(SendResourceFailure),
    NodeStopped,
}

/// RNS 1.4.0 `Resource.AUTO_COMPRESS_MAX_SIZE`
pub const AUTO_COMPRESS_MAX_LEN: u64 = 64 * 1024 * 1024;

pub(super) const fn resource_segment_decompression_bound(uncompressed_data_len: u64) -> u64 {
    if uncompressed_data_len < MAX_EFFICIENT_SIZE as u64 {
        uncompressed_data_len
    } else {
        MAX_EFFICIENT_SIZE as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentCompression {
    Attempt { up_to_byte_len: u64 },
    Never,
}

impl SegmentCompression {
    /// The reference's `auto_compress=True`: attempt up to [`AUTO_COMPRESS_MAX_LEN`].
    pub const AUTO: Self = Self::Attempt {
        up_to_byte_len: AUTO_COMPRESS_MAX_LEN,
    };
}

/// The bytes themselves were streamed to the caller's sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReceipt {
    pub original_hash: ResourceHash,
    pub total_size: u64,
    /// The transfer's packed metadata (msgpack the app unpacks), when one traveled.
    pub metadata: Option<std::vec::Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceProgress {
    pub transferred: u64,
    pub total: u64,
    pub physical_transferred: u64,
    pub segment_index: u64,
    pub total_segments: u64,
}

pub struct PreparedResourceReceiver {
    inbound: mpsc::UnboundedReceiver<ResourceInbound>,
}

#[derive(Debug)]
pub enum ResourceReceiveError {
    Sink(std::io::Error),
    Failed,
    NodeStopped,
}

const ENGINE_SEGMENT_LANES: usize = 2;

struct PendingSegment {
    settled: oneshot::Receiver<Settlement>,
    logical_len: u64,
    physical_len: u64,
    segment_index: u64,
}

pub(super) struct ResourceStreamOptions {
    pub(super) packed_metadata: Option<Arc<[u8]>>,
    pub(super) compression: SegmentCompression,
    pub(super) answers_request: Option<RequestId>,
    pub(super) progress: Option<mpsc::UnboundedSender<ResourceProgress>>,
}

async fn settle_sent_segment(
    settled: oneshot::Receiver<Settlement>,
    answers_request: bool,
) -> Result<(), ResourceSendError> {
    match (answers_request, settled.await) {
        (false, Ok(Settlement::SendResource(Ok(())))) | (true, Ok(Settlement::Respond(Ok(())))) => {
            Ok(())
        }
        (false, Ok(Settlement::SendResource(Err(failure))))
        | (true, Ok(Settlement::Respond(Err(RespondFailure::Resource(failure))))) => {
            Err(ResourceSendError::Rejected(failure))
        }
        (_, Ok(_) | Err(_)) => Err(ResourceSendError::NodeStopped),
    }
}

impl PrnsNodeHandle {
    /// The length is explicit because every segment advertises the total up front; a payload at or under one segment crosses unsplit.
    pub async fn send_resource(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            ResourceStreamOptions {
                packed_metadata: None,
                compression: SegmentCompression::AUTO,
                answers_request: None,
                progress: None,
            },
        )
        .await
    }

    /// The RNS 1.4.0 `auto_compress` parameter: [`SegmentCompression::Never`] ships every segment uncompressed where the default attempts bz2 per segment.
    pub async fn send_resource_with_compression(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
        compression: SegmentCompression,
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            ResourceStreamOptions {
                packed_metadata: None,
                compression,
                answers_request: None,
                progress: None,
            },
        )
        .await
    }

    /// `packed_metadata` is msgpack the peer's app unpacks, opaque all the way down. The block rides ahead of the data in segment one's stream and inside the advertised total, so segment one carries that much less data and a payload near the segment boundary may split one segment sooner.
    pub async fn send_resource_with_metadata(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
        packed_metadata: &[u8],
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            ResourceStreamOptions {
                packed_metadata: Some(packed_metadata.into()),
                compression: SegmentCompression::AUTO,
                answers_request: None,
                progress: None,
            },
        )
        .await
    }

    pub async fn send_resource_with_options(
        &self,
        link_id: LinkId,
        total_len: u64,
        source: impl AsyncRead + Unpin,
        packed_metadata: &[u8],
        compression: SegmentCompression,
        progress: mpsc::UnboundedSender<ResourceProgress>,
    ) -> Result<(), ResourceSendError> {
        self.send_resource_streaming(
            link_id,
            total_len,
            source,
            ResourceStreamOptions {
                packed_metadata: Some(packed_metadata.into()),
                compression,
                answers_request: None,
                progress: Some(progress),
            },
        )
        .await
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.resource.send",
            level = "debug",
            skip_all,
            fields(bytes = total_len, link_id = ?link_id.as_bytes()),
            err(Debug)
        )
    )]
    pub(super) async fn send_resource_streaming(
        &self,
        link_id: LinkId,
        total_len: u64,
        mut source: impl AsyncRead + Unpin,
        options: ResourceStreamOptions,
    ) -> Result<(), ResourceSendError> {
        let ResourceStreamOptions {
            packed_metadata,
            compression,
            answers_request,
            progress,
        } = options;
        let segment_size = MAX_EFFICIENT_SIZE as u64;
        let block_len = match packed_metadata.as_ref() {
            None => 0,
            Some(packed) => u64::try_from(packed.len())
                .ok()
                .and_then(|len| len.checked_add(METADATA_PREFIX_LEN as u64))
                .ok_or(ResourceSendError::UnrepresentableLength)?,
        };
        let stream_total_len = total_len
            .checked_add(block_len)
            .ok_or(ResourceSendError::UnrepresentableLength)?;
        let total_segments = stream_total_len.div_ceil(segment_size).max(1);
        let final_stream_len = stream_total_len % segment_size;
        let rebalance_final_pair =
            total_segments > 1 && final_stream_len != 0 && final_stream_len < segment_size / 2;
        let mut remaining = total_len;
        let mut in_flight: VecDeque<PendingSegment> = VecDeque::with_capacity(ENGINE_SEGMENT_LANES);
        let mut transferred = 0u64;
        let mut physical_transferred = 0u64;
        for segment_index in 1..=total_segments {
            let stream_remaining = remaining + if segment_index == 1 { block_len } else { 0 };
            // RNS emits a proof before removing the completed incoming segment. A very small
            // successor can therefore cross while the peer still has both receivers registered;
            // its single untagged part may be consumed by the retiring receiver and not retried
            // before the link goes stale. Keep the reference's full-sized leading segments, but
            // split a sub-half-segment tail evenly across the final pair.
            let segment_stream_len = if rebalance_final_pair && segment_index == total_segments - 1
            {
                stream_remaining
                    .div_ceil(2)
                    .max(if segment_index == 1 { block_len } else { 0 })
            } else {
                stream_remaining.min(segment_size)
            };
            let capacity = if segment_index == 1 {
                segment_stream_len.saturating_sub(block_len)
            } else {
                segment_stream_len
            };
            let this_segment = remaining.min(capacity);
            remaining -= this_segment;
            let mut chunk = std::vec![0u8; this_segment as usize];
            source
                .read_exact(&mut chunk)
                .await
                .map_err(ResourceSendError::Source)?;
            let first_segment_block = (segment_index == 1)
                .then(|| packed_metadata.clone())
                .flatten();
            let segment_payload_len = if segment_index == 1 {
                block_len + this_segment
            } else {
                this_segment
            };
            let attempt = match compression {
                SegmentCompression::Attempt {
                    up_to_byte_len: up_to,
                } => segment_payload_len <= up_to,
                SegmentCompression::Never => false,
            };
            let (chunk, compressed_candidate) = if attempt {
                tokio::task::spawn_blocking(move || {
                    let candidate = match &first_segment_block {
                        Some(packed) => {
                            let mut composite = Vec::with_capacity(
                                METADATA_PREFIX_LEN + packed.len() + chunk.len(),
                            );
                            composite.extend_from_slice(&(packed.len() as u32).to_be_bytes()[1..]);
                            composite.extend_from_slice(packed);
                            composite.extend_from_slice(&chunk);
                            compression::compress_if_smaller(&composite)
                                .map(HostResourcePayload::from)
                        }
                        None => {
                            compression::compress_if_smaller(&chunk).map(HostResourcePayload::from)
                        }
                    };
                    (chunk, candidate)
                })
                .await
                .map_err(|_| ResourceSendError::NodeStopped)?
            } else {
                (chunk, None)
            };
            let metadata = match (&packed_metadata, segment_index) {
                (None, _) => HostResourceMetadata::None,
                (Some(packed), 1) => HostResourceMetadata::Packed(packed.clone().into()),
                (Some(packed), _) => HostResourceMetadata::SentInFirstSegment {
                    packed_len: packed.len() as u32,
                },
            };
            if in_flight.len() == ENGINE_SEGMENT_LANES {
                if let Some(pending) = in_flight.pop_front() {
                    settle_sent_segment(pending.settled, answers_request.is_some()).await?;
                    transferred = transferred.saturating_add(pending.logical_len);
                    physical_transferred =
                        physical_transferred.saturating_add(pending.physical_len);
                    if let Some(progress) = &progress {
                        let _ = progress.send(ResourceProgress {
                            transferred,
                            total: stream_total_len,
                            physical_transferred,
                            segment_index: pending.segment_index,
                            total_segments,
                        });
                    }
                }
            }
            let physical_len = sealed_transfer_len(
                compressed_candidate
                    .as_ref()
                    .map_or(segment_payload_len as usize, HostResourcePayload::len),
            ) as u64;
            let id = self.mint();
            let (completion, settled) = oneshot::channel();
            self.commands
                .send(HostCommand::SendResourceSegment(
                    SendResourceSegmentHostCommand {
                        id,
                        link_id,
                        data: chunk.into(),
                        compressed_candidate,
                        metadata,
                        request_id: answers_request,
                        segment_index,
                        total_segments,
                        total_data_size: total_len,
                        completion,
                    },
                ))
                .map_err(|_| ResourceSendError::NodeStopped)?;
            in_flight.push_back(PendingSegment {
                settled,
                logical_len: segment_payload_len,
                physical_len,
                segment_index,
            });
        }
        for pending in in_flight {
            settle_sent_segment(pending.settled, answers_request.is_some()).await?;
            transferred = transferred.saturating_add(pending.logical_len);
            physical_transferred = physical_transferred.saturating_add(pending.physical_len);
            if let Some(progress) = &progress {
                let _ = progress.send(ResourceProgress {
                    transferred,
                    total: stream_total_len,
                    physical_transferred,
                    segment_index: pending.segment_index,
                    total_segments,
                });
            }
        }
        Ok(())
    }

    pub async fn prepare_resource_receiver(
        &self,
        link_id: LinkId,
    ) -> Result<PreparedResourceReceiver, ResourceReceiveError> {
        let (chunks, inbound) = mpsc::unbounded_channel();
        let (ready, registered) = oneshot::channel();
        self.commands
            .send(HostCommand::RegisterResourceSink {
                link_id,
                sink: chunks,
                ready,
            })
            .map_err(|_| ResourceReceiveError::NodeStopped)?;
        registered
            .await
            .map_err(|_| ResourceReceiveError::NodeStopped)?;
        Ok(PreparedResourceReceiver { inbound })
    }

    /// Registers the sink before yielding, so a segment arriving the instant after cannot reach the app event stream instead.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.resource.receive",
            level = "debug",
            skip_all,
            fields(link_id = ?link_id.as_bytes()),
            err(Debug)
        )
    )]
    pub async fn receive_resource(
        &self,
        link_id: LinkId,
        sink: impl AsyncWrite + Unpin,
    ) -> Result<ResourceReceipt, ResourceReceiveError> {
        self.prepare_resource_receiver(link_id)
            .await?
            .receive(sink)
            .await
    }

    pub async fn set_link_resource_strategy(
        &self,
        link_id: LinkId,
        strategy: ResourceStrategy,
    ) -> Result<(), crate::runtime::SendError<crate::engine::SetResourceStrategyFailure>> {
        match self
            .settle(crate::engine::EngineCommand::SetResourceStrategy(
                crate::engine::SetResourceStrategy { link_id, strategy },
            ))
            .await
        {
            Some(Settlement::SetResourceStrategy(result)) => {
                result.map_err(crate::runtime::SendError::Failed)
            }
            Some(_) | None => Err(crate::runtime::SendError::NodeStopped),
        }
    }

    pub async fn set_resource_strategy(
        &self,
        destination: DestinationHash,
        strategy: ResourceStrategy,
    ) -> bool {
        let (ready, applied) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SetResourceStrategy {
                destination,
                strategy,
                ready,
            })
            .is_err()
        {
            return false;
        }
        applied.await.unwrap_or(false)
    }
}

impl PreparedResourceReceiver {
    pub async fn receive(
        mut self,
        mut sink: impl AsyncWrite + Unpin,
    ) -> Result<ResourceReceipt, ResourceReceiveError> {
        let mut metadata = None;
        loop {
            match self.inbound.recv().await {
                Some(ResourceInbound::Metadata(packed)) => metadata = Some(packed),
                Some(ResourceInbound::Chunk(bytes)) => {
                    sink.write_all(&bytes)
                        .await
                        .map_err(ResourceReceiveError::Sink)?;
                }
                Some(ResourceInbound::Complete {
                    original_hash,
                    total_size,
                }) => {
                    sink.flush().await.map_err(ResourceReceiveError::Sink)?;
                    return Ok(ResourceReceipt {
                        original_hash,
                        total_size,
                        metadata,
                    });
                }
                Some(ResourceInbound::Failed) => return Err(ResourceReceiveError::Failed),
                None => return Err(ResourceReceiveError::NodeStopped),
            }
        }
    }
}

#[cfg(test)]
mod tests;
