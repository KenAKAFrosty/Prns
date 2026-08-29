use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::oneshot;

use crate::engine::RequestResponseTimeout;
use crate::engine::RespondFailure;
use crate::engine::SendRequestFailure;
use crate::engine::Settlement;
use crate::manifold::compression;
use crate::manifold::driver::{
    HostCommand, HostResourcePayload, RequestAnyHostCommand, RespondAnyHostCommand,
};
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::{
    packed_binary_len, response_envelope_prefix, write_packed_binary_header,
    write_response_plaintext, MAX_PACKED_BINARY_HEADER_LEN, RESPONSE_WIRE_OVERHEAD,
};
use crate::routing::links::resources::send::STATIC_RESPONSE_SEGMENT_BYTES;
use crate::routing::links::resources::MAX_EFFICIENT_SIZE;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::ByteLimit;
use crate::units::RttMillis;

use super::super::request_endpoints::RespondToken;
use super::super::SendError;
use super::resource_transfer::{
    ResourceSendError, ResourceStreamOptions, SegmentCompression, ENGINE_SEGMENT_LANES,
};
use super::PrnsNodeHandle;
use prns_core::rncp::write_file_metadata;

const RESPONSE_PACKET_CEILING: usize = LINK_MDU - RESPONSE_WIRE_OVERHEAD;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RequestOptions {
    pub response_timeout: RequestResponseTimeout,
    pub maximum_response_bytes: ByteLimit,
}

#[derive(Debug)]
pub enum ResponseSendError {
    Source(std::io::Error),
    UnrepresentableLength,
    NodeStopped,
    CompressionTask,
    Rejected(RespondFailure),
    UnexpectedSettlement,
}

impl std::fmt::Display for ResponseSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(error) => write!(f, "could not read the response source: {error}"),
            Self::UnrepresentableLength => {
                f.write_str("the response length cannot be represented on the wire")
            }
            Self::NodeStopped => f.write_str("the node stopped before the response settled"),
            Self::CompressionTask => f.write_str("the response compression task stopped"),
            Self::Rejected(error) => write!(f, "response failed: {error:?}"),
            Self::UnexpectedSettlement => f.write_str("response returned an unrelated settlement"),
        }
    }
}

impl PrnsNodeHandle {
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "prns.request",
            level = "debug",
            skip_all,
            fields(bytes = data.len(), link_id = ?link_id.as_bytes(), path_hash = ?path_hash),
            err(Debug)
        )
    )]
    pub async fn request(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
    ) -> Result<(std::vec::Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        self.request_with_response_timeout(
            link_id,
            path_hash,
            data,
            RequestResponseTimeout::LinkDefault,
        )
        .await
    }

    pub async fn request_with_response_timeout(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
        response_timeout: RequestResponseTimeout,
    ) -> Result<(std::vec::Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        self.request_with_options(
            link_id,
            path_hash,
            data,
            RequestOptions {
                response_timeout,
                maximum_response_bytes: ByteLimit::Unlimited,
            },
        )
        .await
    }

    pub async fn request_with_options(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
        options: RequestOptions,
    ) -> Result<(std::vec::Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        self.request_owned_with_options(link_id, path_hash, data.to_vec(), options)
            .await
    }

    pub(super) async fn request_owned_with_options(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: std::vec::Vec<u8>,
        options: RequestOptions,
    ) -> Result<(std::vec::Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::RequestAny(RequestAnyHostCommand {
                id,
                link_id,
                path_hash,
                data: data.into(),
                response_timeout: options.response_timeout,
                maximum_response_bytes: options.maximum_response_bytes,
                completion,
            }))
            .map_err(|_| SendError::NodeStopped)?;
        match settled.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(failure)) => Err(SendError::Failed(failure)),
            Err(_) => Err(SendError::NodeStopped),
        }
    }

    fn send_packed_response(
        &self,
        responder: RespondToken,
        packed: HostResourcePayload,
    ) -> Option<RttMillis> {
        let id = self.mint();
        if packed.len() <= RESPONSE_PACKET_CEILING {
            return self
                .commands
                .send(HostCommand::RespondAny(RespondAnyHostCommand {
                    id,
                    link_id: responder.link_id,
                    request_id: responder.request_id,
                    packed,
                    compressed_candidate: None,
                    completion: None,
                }))
                .ok()
                .map(|()| responder.rtt);
        }
        if self.commands.is_closed() {
            return None;
        }
        let response_capacity = RESPONSE_WIRE_OVERHEAD.checked_add(packed.len().max(1))?;
        let mut response = std::vec![0u8; response_capacity];
        let response_len = write_response_plaintext(
            &responder.request_id,
            packed.as_slice(),
            response.as_mut_slice(),
        )
        .ok()?;
        response.truncate(response_len);
        let packed = HostResourcePayload::from(response);
        if packed.len() > MAX_EFFICIENT_SIZE {
            let handle = self.clone();
            let link_id = responder.link_id;
            let request_id = responder.request_id;
            tokio::spawn(async move {
                let total_len = packed.len() as u64;
                let _ = handle
                    .send_resource_streaming(
                        link_id,
                        total_len,
                        std::io::Cursor::new(packed),
                        ResourceStreamOptions {
                            packed_metadata: None,
                            compression: SegmentCompression::AUTO,
                            answers_request: Some(request_id),
                            progress: None,
                            segment_size: MAX_EFFICIENT_SIZE as u64,
                            max_in_flight_segments: ENGINE_SEGMENT_LANES,
                        },
                    )
                    .await;
            });
            return Some(responder.rtt);
        }
        let commands = self.commands.clone();
        let link_id = responder.link_id;
        let request_id = responder.request_id;
        tokio::spawn(async move {
            let Ok((packed, compressed_candidate)) = tokio::task::spawn_blocking(move || {
                let candidate = compression::compress_if_smaller(packed.as_slice())
                    .map(HostResourcePayload::from);
                (packed, candidate)
            })
            .await
            else {
                return;
            };
            let _ = commands.send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id,
                request_id,
                packed,
                compressed_candidate,
                completion: None,
            }));
        });
        Some(responder.rtt)
    }

    /// Answer a request via its token, returning the link's round trip (the request arrived over it) — or `None` if the node has stopped before the answer could be queued.
    pub fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> Option<RttMillis> {
        self.send_packed_response(responder, packed.to_vec().into())
    }

    pub fn respond_owned_packed(
        &self,
        responder: RespondToken,
        packed: std::vec::Vec<u8>,
    ) -> Option<RttMillis> {
        self.send_packed_response(responder, packed.into())
    }

    pub fn respond_bytes(&self, responder: RespondToken, bytes: &[u8]) -> Option<RttMillis> {
        let mut packed = std::vec::Vec::with_capacity(packed_binary_len(bytes.len())?);
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(bytes.len(), &mut header).ok()?;
        packed.extend_from_slice(&header[..header_len]);
        packed.extend_from_slice(bytes);
        self.respond_owned_packed(responder, packed)
    }

    pub fn respond_owned_bytes(
        &self,
        responder: RespondToken,
        bytes: std::vec::Vec<u8>,
    ) -> Option<RttMillis> {
        let packed_len = packed_binary_len(bytes.len())?;
        let mut packed = std::vec::Vec::with_capacity(packed_len);
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(bytes.len(), &mut header).ok()?;
        packed.extend_from_slice(&header[..header_len]);
        packed.extend_from_slice(&bytes);
        self.respond_owned_packed(responder, packed)
    }

    pub async fn respond_bytes_streaming(
        &self,
        responder: RespondToken,
        byte_len: u64,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<RttMillis, ResponseSendError> {
        let byte_len_usize =
            usize::try_from(byte_len).map_err(|_| ResponseSendError::UnrepresentableLength)?;
        let packed_len =
            packed_binary_len(byte_len_usize).ok_or(ResponseSendError::UnrepresentableLength)?;
        if packed_len <= RESPONSE_PACKET_CEILING {
            let mut bytes = std::vec![0u8; byte_len_usize];
            source
                .read_exact(&mut bytes)
                .await
                .map_err(ResponseSendError::Source)?;
            let mut packed = std::vec::Vec::with_capacity(packed_len);
            let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
            let header_len = write_packed_binary_header(byte_len_usize, &mut header)
                .map_err(|_| ResponseSendError::UnrepresentableLength)?;
            packed.extend_from_slice(&header[..header_len]);
            packed.extend_from_slice(&bytes);
            return self.respond_owned_packed_settled(responder, packed).await;
        }

        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(byte_len_usize, &mut header)
            .map_err(|_| ResponseSendError::UnrepresentableLength)?;
        let mut prefix = std::vec::Vec::with_capacity(RESPONSE_WIRE_OVERHEAD + header_len);
        prefix.extend_from_slice(&response_envelope_prefix(&responder.request_id));
        prefix.extend_from_slice(&header[..header_len]);
        let response_len = u64::try_from(prefix.len())
            .ok()
            .and_then(|prefix_len| prefix_len.checked_add(byte_len))
            .ok_or(ResponseSendError::UnrepresentableLength)?;
        self.send_resource_streaming(
            responder.link_id,
            response_len,
            std::io::Cursor::new(prefix).chain(source),
            ResourceStreamOptions {
                packed_metadata: None,
                compression: SegmentCompression::AUTO,
                answers_request: Some(responder.request_id),
                progress: None,
                segment_size: MAX_EFFICIENT_SIZE as u64,
                max_in_flight_segments: ENGINE_SEGMENT_LANES,
            },
        )
        .await
        .map_err(|error| match error {
            ResourceSendError::Source(error) => ResponseSendError::Source(error),
            ResourceSendError::UnrepresentableLength => ResponseSendError::UnrepresentableLength,
            ResourceSendError::Rejected(error) => {
                ResponseSendError::Rejected(RespondFailure::Resource(error))
            }
            ResourceSendError::NodeStopped => ResponseSendError::NodeStopped,
        })?;
        Ok(responder.rtt)
    }

    /// Send a static response as a NomadNet-compatible named file without copying the complete
    /// payload. Each segment is bounded to 256 KiB and the next segment is not read until the
    /// current resource proof settles. RNS 1.4.2 hands a metadata-bearing response to the
    /// requester as the raw resource payload, so the bytes travel bare: no response envelope,
    /// no binary header.
    pub(crate) async fn respond_static_file_settled(
        &self,
        responder: RespondToken,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<RttMillis, ResponseSendError> {
        let mut packed_metadata = [0u8; 6 + 2 + u8::MAX as usize];
        let metadata_len = write_file_metadata(name.as_bytes(), &mut packed_metadata)
            .map_err(|_| ResponseSendError::UnrepresentableLength)?;
        let response_len =
            u64::try_from(bytes.len()).map_err(|_| ResponseSendError::UnrepresentableLength)?;

        self.send_resource_streaming(
            responder.link_id,
            response_len,
            std::io::Cursor::new(bytes),
            ResourceStreamOptions {
                packed_metadata: Some(packed_metadata[..metadata_len].into()),
                compression: SegmentCompression::AUTO,
                answers_request: Some(responder.request_id),
                progress: None,
                segment_size: STATIC_RESPONSE_SEGMENT_BYTES as u64,
                max_in_flight_segments: 1,
            },
        )
        .await
        .map_err(|error| match error {
            ResourceSendError::Source(error) => ResponseSendError::Source(error),
            ResourceSendError::UnrepresentableLength => ResponseSendError::UnrepresentableLength,
            ResourceSendError::Rejected(error) => {
                ResponseSendError::Rejected(RespondFailure::Resource(error))
            }
            ResourceSendError::NodeStopped => ResponseSendError::NodeStopped,
        })?;
        Ok(responder.rtt)
    }

    /// Send an already-open file as a NomadNet-compatible named response. The handle is consumed
    /// only after the request runner acquires the link's response lane, and at most one 256 KiB
    /// segment is retained while the resource proof settles.
    pub(crate) async fn respond_open_file_settled(
        &self,
        responder: RespondToken,
        name: &str,
        file: std::fs::File,
        byte_len: u64,
    ) -> Result<RttMillis, ResponseSendError> {
        let mut packed_metadata = [0u8; 6 + 2 + u8::MAX as usize];
        let metadata_len = write_file_metadata(name.as_bytes(), &mut packed_metadata)
            .map_err(|_| ResponseSendError::UnrepresentableLength)?;

        self.send_resource_streaming(
            responder.link_id,
            byte_len,
            tokio::fs::File::from_std(file),
            ResourceStreamOptions {
                packed_metadata: Some(packed_metadata[..metadata_len].into()),
                compression: SegmentCompression::AUTO,
                answers_request: Some(responder.request_id),
                progress: None,
                segment_size: STATIC_RESPONSE_SEGMENT_BYTES as u64,
                max_in_flight_segments: 1,
            },
        )
        .await
        .map_err(|error| match error {
            ResourceSendError::Source(error) => ResponseSendError::Source(error),
            ResourceSendError::UnrepresentableLength => ResponseSendError::UnrepresentableLength,
            ResourceSendError::Rejected(error) => {
                ResponseSendError::Rejected(RespondFailure::Resource(error))
            }
            ResourceSendError::NodeStopped => ResponseSendError::NodeStopped,
        })?;
        Ok(responder.rtt)
    }

    pub(crate) async fn respond_owned_packed_settled(
        &self,
        responder: RespondToken,
        packed: std::vec::Vec<u8>,
    ) -> Result<RttMillis, ResponseSendError> {
        let id = self.mint();
        let packed = HostResourcePayload::from(packed);
        if packed.len() > RESPONSE_PACKET_CEILING {
            let packed_capacity = RESPONSE_WIRE_OVERHEAD
                .checked_add(packed.len().max(1))
                .ok_or(ResponseSendError::UnrepresentableLength)?;
            let mut response = std::vec![0u8; packed_capacity];
            let response_len = write_response_plaintext(
                &responder.request_id,
                packed.as_slice(),
                response.as_mut_slice(),
            )
            .map_err(|_| ResponseSendError::UnexpectedSettlement)?;
            response.truncate(response_len);
            let packed = HostResourcePayload::from(response);
            if packed.len() > MAX_EFFICIENT_SIZE {
                return self
                    .send_resource_streaming(
                        responder.link_id,
                        packed.len() as u64,
                        std::io::Cursor::new(packed),
                        ResourceStreamOptions {
                            packed_metadata: None,
                            compression: SegmentCompression::AUTO,
                            answers_request: Some(responder.request_id),
                            progress: None,
                            segment_size: MAX_EFFICIENT_SIZE as u64,
                            max_in_flight_segments: ENGINE_SEGMENT_LANES,
                        },
                    )
                    .await
                    .map(|()| responder.rtt)
                    .map_err(|error| match error {
                        ResourceSendError::Rejected(error) => {
                            ResponseSendError::Rejected(RespondFailure::Resource(error))
                        }
                        ResourceSendError::Source(error) => ResponseSendError::Source(error),
                        ResourceSendError::UnrepresentableLength => {
                            ResponseSendError::UnrepresentableLength
                        }
                        ResourceSendError::NodeStopped => ResponseSendError::NodeStopped,
                    });
            }
            let (packed, compressed_candidate) = tokio::task::spawn_blocking(move || {
                let candidate = compression::compress_if_smaller(packed.as_slice())
                    .map(HostResourcePayload::from);
                (packed, candidate)
            })
            .await
            .map_err(|_| ResponseSendError::CompressionTask)?;
            return self
                .send_response_command_settled(id, responder, packed, compressed_candidate)
                .await;
        }
        self.send_response_command_settled(id, responder, packed, None)
            .await
    }

    async fn send_response_command_settled(
        &self,
        id: crate::engine::CommandId,
        responder: RespondToken,
        packed: HostResourcePayload,
        compressed_candidate: Option<HostResourcePayload>,
    ) -> Result<RttMillis, ResponseSendError> {
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                packed,
                compressed_candidate,
                completion: Some(completion),
            }))
            .map_err(|_| ResponseSendError::NodeStopped)?;
        match settled.await {
            Ok(Settlement::Respond(Ok(()))) => Ok(responder.rtt),
            Ok(Settlement::Respond(Err(error))) => Err(ResponseSendError::Rejected(error)),
            Ok(_) => Err(ResponseSendError::UnexpectedSettlement),
            Err(_) => Err(ResponseSendError::NodeStopped),
        }
    }
}

#[cfg(test)]
mod tests;
