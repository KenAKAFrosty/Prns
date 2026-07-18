//! Platform-neutral command-surface types, shared by every command handle: a `send_single_packet` resolves to the same `Result` whether Tokio's oneshot or Embassy's completion pool carried the settlement.

use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, PacketReceiptDelivered,
    SendSinglePacketFailure,
};
use crate::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::routing::links::LinkId;
pub use crate::routing::{ClearAnnounceQueuesOutcome, DropRouteOutcome, DropRoutesViaOutcome};
use crate::wire::{DestinationHash, TransportId};

use super::request_router::RespondToken;

/// Why an awaited send never reached `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError<F> {
    /// The payload was larger than a single packet's MDU — rejected before the wire.
    PayloadTooLarge,
    /// The node has stopped: the command channel is closed (host) or the bounded lane is gone.
    NodeStopped,
    /// More awaited sends are in flight than the platform tracks at once — the embedded
    /// `CompletionPool` is full. The unbounded host path never returns it.
    Busy,
    /// The engine settled the send as a typed failure (`SendSinglePacketFailure`, …).
    Failed(F),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingControlError {
    NodeStopped,
    Busy,
}

pub trait RoutingControl {
    fn drop_route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send;

    fn drop_routes_via(
        &self,
        transport: TransportId,
    ) -> impl core::future::Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send;

    fn clear_announce_queues(
        &self,
    ) -> impl core::future::Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationIdentityRetentionControlError {
    NodeStopped,
    Busy,
}

pub trait DestinationIdentityRetentionControl {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<MarkDestinationUsedOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<RetainDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<
        Output = Result<ReleaseDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send;

    fn retain_identity(
        &self,
        identity: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<RetainIdentityOutcome, DestinationIdentityRetentionControlError>,
    > + Send;
}

/// The high-level API shared by every platform's node handle. Tokio carries commands over an unbounded channel with per-command oneshots; Embassy uses a bounded channel and static completion pool. [`issue`](Self::issue) returns a minted [`CommandId`] immediately, while awaited operations settle through that id. Platform-specific capabilities remain inherent methods on the concrete handle.
#[allow(async_fn_in_trait)]
pub trait PrnsNodeApi {
    /// Queue an engine command and return the [`CommandId`] it was minted under — watch the event
    /// stream for the settlement tagged with it. `None` once the node has stopped (or the bounded
    /// embedded lane is full). The fire-and-forget escape hatch.
    fn issue(&self, command: EngineCommand) -> Option<CommandId>;

    /// Announce `destination` on every interface with its registered app_data: RNS 1.3.5
    /// `Destination.announce()` with no arguments. `None` once the node has stopped. For one
    /// interface or explicit app_data, [`issue`](Self::issue) a custom [`AnnounceNow`].
    fn announce(&self, destination: DestinationHash) -> Option<CommandId> {
        self.issue(EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }))
    }

    /// Send one Single data packet to `destination` and await its delivery proof — `Ok(Delivered)`
    /// with the measured round trip, or the typed reason it did not deliver.
    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>>;

    /// Answer a request with `body`. Returns `false` once the node has stopped (or, on embedded, if
    /// `body` exceeds the single-packet MDU the inline responder can carry).
    fn respond(&self, responder: RespondToken, body: &[u8]) -> bool;

    /// Sever an active link. Returns `false` once the node has stopped.
    fn close_link(&self, link_id: LinkId) -> bool;
}
