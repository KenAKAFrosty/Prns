use tokio::sync::oneshot;

use crate::engine::RequestResponseTimeout;
use crate::engine::RespondFailure;
use crate::engine::SendRequestFailure;
use crate::engine::SendResourceFailure;
use crate::engine::Settlement;
use crate::reactor::compression;
use crate::reactor::driver::{
    HostCommand, HostResourcePayload, RequestAnyHostCommand, RespondAnyHostCommand,
};
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::{write_response_plaintext, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::resources::MAX_EFFICIENT_SIZE;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;

use super::super::request_router::RespondToken;
use super::super::SendError;
use super::resource_transfer::{ResourceSendError, ResourceStreamOptions, SegmentCompression};
use super::PrnsNodeHandle;

const RESPONSE_PACKET_CEILING: usize = LINK_MDU - RESPONSE_WIRE_OVERHEAD;

#[derive(Debug)]
pub(crate) enum ResponseSettlementError {
    NodeStopped,
    CompressionTask,
    Respond(RespondFailure),
    Resource(SendResourceFailure),
    UnexpectedSettlement,
}

impl std::fmt::Display for ResponseSettlementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeStopped => f.write_str("the node stopped before the response settled"),
            Self::CompressionTask => f.write_str("the response compression task stopped"),
            Self::Respond(error) => write!(f, "response packet failed: {error:?}"),
            Self::Resource(error) => write!(f, "response resource failed: {error:?}"),
            Self::UnexpectedSettlement => f.write_str("response returned an unrelated settlement"),
        }
    }
}

impl PrnsNodeHandle {
    /// Make a request of `path_hash` with `data` of any length and await the response. The runtime picks the rung (a single REQUEST packet within the link MDU, or a resource that rides past it), so a consumer never meets a size limit; the answer carries the measured round trip.
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
        let id = self.mint();
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::RequestAny(RequestAnyHostCommand {
                id,
                link_id,
                path_hash,
                data: data.to_vec().into(),
                response_timeout,
                completion,
            }))
            .map_err(|_| SendError::NodeStopped)?;
        match settled.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(failure)) => Err(SendError::Failed(failure)),
            Err(_) => Err(SendError::NodeStopped),
        }
    }

    fn send_response(
        &self,
        responder: RespondToken,
        data: HostResourcePayload,
    ) -> Option<RttMillis> {
        let id = self.mint();
        if data.len() <= RESPONSE_PACKET_CEILING {
            return self
                .commands
                .send(HostCommand::RespondAny(RespondAnyHostCommand {
                    id,
                    link_id: responder.link_id,
                    request_id: responder.request_id,
                    data,
                    compressed_candidate: None,
                    completion: None,
                }))
                .ok()
                .map(|()| responder.rtt);
        }
        if self.commands.is_closed() {
            return None;
        }
        let packed_capacity = RESPONSE_WIRE_OVERHEAD.checked_add(data.len().max(1))?;
        let mut packed = std::vec![0u8; packed_capacity];
        let packed_len = write_response_plaintext(
            &responder.request_id,
            data.as_slice(),
            packed.as_mut_slice(),
        )
        .ok()?;
        packed.truncate(packed_len);
        let data = HostResourcePayload::from(packed);
        if data.len() > MAX_EFFICIENT_SIZE {
            let handle = self.clone();
            let link_id = responder.link_id;
            let request_id = responder.request_id;
            tokio::spawn(async move {
                let total_len = data.len() as u64;
                let _ = handle
                    .send_resource_streaming(
                        link_id,
                        total_len,
                        std::io::Cursor::new(data),
                        ResourceStreamOptions {
                            packed_metadata: None,
                            compression: SegmentCompression::AUTO,
                            answers_request: Some(request_id),
                            progress: None,
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
            let Ok((data, compressed_candidate)) = tokio::task::spawn_blocking(move || {
                let candidate = compression::compress_if_smaller(data.as_slice())
                    .map(HostResourcePayload::from);
                (data, candidate)
            })
            .await
            else {
                return;
            };
            let _ = commands.send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id,
                request_id,
                data,
                compressed_candidate,
                completion: None,
            }));
        });
        Some(responder.rtt)
    }

    /// Answer a request via its token, returning the link's round trip (the request arrived over it) — or `None` if the node has stopped before the answer could be queued.
    pub fn respond(&self, responder: RespondToken, body: &[u8]) -> Option<RttMillis> {
        self.send_response(responder, body.to_vec().into())
    }

    pub fn respond_owned(
        &self,
        responder: RespondToken,
        body: std::vec::Vec<u8>,
    ) -> Option<RttMillis> {
        self.send_response(responder, body.into())
    }

    /// Queue a response and hold the caller until its packet write or Resource proof settles.
    /// The request router uses this to serialize Resource responses per link: a requester may
    /// issue its next request immediately after sending the previous Resource proof, before this
    /// node has processed that proof and released the one-resource-per-link lane.
    pub(crate) async fn respond_owned_settled(
        &self,
        responder: RespondToken,
        body: std::vec::Vec<u8>,
    ) -> Result<RttMillis, ResponseSettlementError> {
        let id = self.mint();
        let data = HostResourcePayload::from(body);
        if data.len() > RESPONSE_PACKET_CEILING {
            let packed_capacity = RESPONSE_WIRE_OVERHEAD
                .checked_add(data.len().max(1))
                .ok_or(ResponseSettlementError::UnexpectedSettlement)?;
            let mut packed = std::vec![0u8; packed_capacity];
            let packed_len = write_response_plaintext(
                &responder.request_id,
                data.as_slice(),
                packed.as_mut_slice(),
            )
            .map_err(|_| ResponseSettlementError::UnexpectedSettlement)?;
            packed.truncate(packed_len);
            let data = HostResourcePayload::from(packed);
            if data.len() > MAX_EFFICIENT_SIZE {
                return self
                    .send_resource_streaming(
                        responder.link_id,
                        data.len() as u64,
                        std::io::Cursor::new(data),
                        ResourceStreamOptions {
                            packed_metadata: None,
                            compression: SegmentCompression::AUTO,
                            answers_request: Some(responder.request_id),
                            progress: None,
                        },
                    )
                    .await
                    .map(|()| responder.rtt)
                    .map_err(|error| match error {
                        ResourceSendError::Rejected(error) => {
                            ResponseSettlementError::Resource(error)
                        }
                        ResourceSendError::Source(_)
                        | ResourceSendError::UnrepresentableLength
                        | ResourceSendError::NodeStopped => ResponseSettlementError::NodeStopped,
                    });
            }
            let (data, compressed_candidate) = tokio::task::spawn_blocking(move || {
                let candidate = compression::compress_if_smaller(data.as_slice())
                    .map(HostResourcePayload::from);
                (data, candidate)
            })
            .await
            .map_err(|_| ResponseSettlementError::CompressionTask)?;
            return self
                .send_response_command_settled(id, responder, data, compressed_candidate)
                .await;
        }
        self.send_response_command_settled(id, responder, data, None)
            .await
    }

    async fn send_response_command_settled(
        &self,
        id: crate::engine::CommandId,
        responder: RespondToken,
        data: HostResourcePayload,
        compressed_candidate: Option<HostResourcePayload>,
    ) -> Result<RttMillis, ResponseSettlementError> {
        let (completion, settled) = oneshot::channel();
        self.commands
            .send(HostCommand::RespondAny(RespondAnyHostCommand {
                id,
                link_id: responder.link_id,
                request_id: responder.request_id,
                data,
                compressed_candidate,
                completion: Some(completion),
            }))
            .map_err(|_| ResponseSettlementError::NodeStopped)?;
        match settled.await {
            Ok(Settlement::Respond(Ok(())) | Settlement::SendResource(Ok(()))) => Ok(responder.rtt),
            Ok(Settlement::Respond(Err(error))) => Err(ResponseSettlementError::Respond(error)),
            Ok(Settlement::SendResource(Err(error))) => {
                Err(ResponseSettlementError::Resource(error))
            }
            Ok(_) => Err(ResponseSettlementError::UnexpectedSettlement),
            Err(_) => Err(ResponseSettlementError::NodeStopped),
        }
    }
}

#[cfg(test)]
mod tests;
