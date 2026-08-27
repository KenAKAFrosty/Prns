use std::future::Future;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::interfaces::PacketPhyStats;
use crate::manifold::driver::HostCommand;
use crate::node_introspection::{
    AnnounceRateSnapshot, DestinationIdentityQuery, DestinationIdentitySnapshot,
    EngineInspectionSnapshot, InterfaceInventoryEntry, NodeIntrospection, NodeIntrospectionRequest,
    RouteSnapshot,
};
use crate::routing::dedup::PacketHash;
use crate::routing::BlackholedIdentity;
use crate::wire::{DestinationHash, TransportId};

use super::super::identity_blackhole_commands::{settle_control, settle_source};
use super::super::settle_destination_identity_retention;
#[cfg(feature = "runtime-metrics")]
use super::super::RuntimeMetricsSnapshot;
use super::super::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DestinationIdentityRetentionHostCommand,
    DropRouteOutcome, DropRoutesViaOutcome, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeHostCommand, IdentityBlackholeSource,
    IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};
use super::PrnsNodeHandle;

impl PrnsNodeHandle {
    #[cfg(feature = "runtime-metrics")]
    pub async fn metrics_snapshot(&self) -> Option<RuntimeMetricsSnapshot> {
        let (reply, snapshot) = oneshot::channel();
        if self
            .commands
            .send(HostCommand::SnapshotMetrics { reply })
            .is_err()
        {
            return None;
        }
        snapshot.await.ok()
    }

    async fn introspect<T>(
        &self,
        request: impl FnOnce(oneshot::Sender<T>) -> NodeIntrospectionRequest,
    ) -> Option<T> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(HostCommand::NodeIntrospection(request(reply)))
            .ok()?;
        response.await.ok()
    }

    pub async fn destination_identity_hash(
        &self,
        destination: DestinationHash,
    ) -> Option<crate::identity::IdentityHash> {
        self.introspect(|reply| NodeIntrospectionRequest::DestinationIdentityHash {
            destination,
            reply,
        })
        .await
        .flatten()
    }

    pub async fn destination_identity(
        &self,
        query: DestinationIdentityQuery,
    ) -> Option<DestinationIdentitySnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::DestinationIdentity { query, reply })
            .await
            .flatten()
    }

    pub async fn destination_identities(&self) -> std::vec::Vec<DestinationIdentitySnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::DestinationIdentities { reply })
            .await
            .unwrap_or_default()
    }

    pub async fn engine_inspection_snapshot(&self) -> Option<EngineInspectionSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::EngineSnapshot { reply })
            .await
    }
}

async fn settle_routing_control<T, F>(
    commands: UnboundedSender<HostCommand>,
    build: F,
) -> Result<T, RoutingControlError>
where
    T: Send,
    F: FnOnce(oneshot::Sender<T>) -> HostCommand + Send,
{
    let (reply, settled) = oneshot::channel();
    commands
        .send(build(reply))
        .map_err(|_| RoutingControlError::NodeStopped)?;
    settled.await.map_err(|_| RoutingControlError::NodeStopped)
}

impl RoutingControl for PrnsNodeHandle {
    fn drop_route(
        &self,
        destination: DestinationHash,
    ) -> impl Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), move |reply| HostCommand::DropRoute {
            destination,
            reply,
        })
    }

    fn drop_routes_via(
        &self,
        transport: TransportId,
    ) -> impl Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), move |reply| {
            HostCommand::DropRoutesVia { transport, reply }
        })
    }

    fn clear_announce_queues(
        &self,
    ) -> impl Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send {
        settle_routing_control(self.commands.clone(), |reply| {
            HostCommand::ClearAnnounceQueues { reply }
        })
    }
}

impl DestinationIdentityRetentionControl for PrnsNodeHandle {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::MarkDestinationUsedOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::MarkUsed { destination, reply }
        })
    }

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainDestinationOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::RetainDestination { destination, reply }
        })
    }

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::ReleaseDestinationOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::ReleaseDestination { destination, reply }
        })
    }

    fn retain_identity(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<
        Output = Result<
            crate::identity::RetainIdentityOutcome,
            DestinationIdentityRetentionControlError,
        >,
    > + Send {
        settle_destination_identity_retention(self.commands.clone(), move |reply| {
            DestinationIdentityRetentionHostCommand::RetainIdentity { identity, reply }
        })
    }
}

impl IdentityBlackholeSource for PrnsNodeHandle {
    type Reason = String;
    type Entries = std::vec::Vec<BlackholedIdentity<String>>;

    fn blackholed_identities(
        &self,
    ) -> impl Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send {
        settle_source(self.commands.clone(), |reply| {
            IdentityBlackholeHostCommand::ReadAll { reply }
        })
    }

    fn is_blackholed(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send {
        settle_source(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::IsBlackholed { identity, reply }
        })
    }
}

impl IdentityBlackholeControl for PrnsNodeHandle {
    fn blackhole_identity<'a>(
        &'a self,
        entry: BlackholedIdentity<&'a str>,
    ) -> impl Future<
        Output = Result<crate::routing::BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send
           + 'a {
        let entry = BlackholedIdentity {
            identity: entry.identity,
            source: entry.source,
            expiry: entry.expiry,
            reason: entry.reason.map(String::from),
        };
        settle_control(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::Blackhole { entry, reply }
        })
    }

    fn unblackhole_identity(
        &self,
        identity: crate::identity::IdentityHash,
    ) -> impl Future<
        Output = Result<crate::routing::UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send {
        settle_control(self.commands.clone(), move |reply| {
            IdentityBlackholeHostCommand::Unblackhole { identity, reply }
        })
    }
}

impl NodeIntrospection for PrnsNodeHandle {
    fn interface_inventory(&self) -> std::vec::Vec<InterfaceInventoryEntry> {
        PrnsNodeHandle::interface_inventory(self)
    }

    fn interface_timing_inventory(
        &self,
    ) -> std::vec::Vec<crate::node_introspection::InterfaceTimingSnapshot> {
        PrnsNodeHandle::interface_timing_inventory(self)
    }

    async fn link_count(&self) -> u32 {
        self.introspect(|reply| NodeIntrospectionRequest::LinkCount { reply })
            .await
            .unwrap_or_default()
    }

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        self.store.packet_phy(packet_hash)
    }

    async fn announce_rates(&self) -> std::vec::Vec<AnnounceRateSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::AnnounceRates { reply })
            .await
            .unwrap_or_default()
    }

    async fn routes(&self) -> std::vec::Vec<RouteSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::Routes { reply })
            .await
            .unwrap_or_default()
    }

    async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
        self.introspect(|reply| NodeIntrospectionRequest::Route { destination, reply })
            .await
            .flatten()
    }
}

#[cfg(test)]
mod tests;
