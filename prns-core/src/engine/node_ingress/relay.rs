#[cfg(feature = "runtime-metrics")]
use crate::engine::{AnnounceOrigin, PathRequestRelayOutcome};
use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, PathRequestIdBytes, ReemitAnnounce,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceKind};
use crate::routing::path_requests::write_path_request_wire_packet;
use crate::routing::timing::{path_request_egress_eligible, PathRequestAudience};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, BROADCAST_MTU};

#[derive(Clone, Copy)]
pub(super) enum RelayAudience {
    AllNetworkInterfaces,
    OnlineNetworkInterfaces,
    BoundaryAndGateway,
    LocalClients,
}

pub(super) struct RelayPathRequest<'a> {
    pub(super) destination: DestinationHash,
    pub(super) id: &'a PathRequestIdBytes,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn relay_path_request(
        &mut self,
        request: RelayPathRequest<'_>,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        audience: RelayAudience,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let mut buf = [0u8; BROADCAST_MTU];
        let transport_id = self
            .network_transport_enabled()
            .then(|| self.transport_id())
            .flatten();
        let Ok(wire_bytes) =
            write_path_request_wire_packet(request.destination, transport_id, request.id, &mut buf)
        else {
            return;
        };
        for descriptor in interfaces {
            let path_audience = match audience {
                RelayAudience::AllNetworkInterfaces | RelayAudience::OnlineNetworkInterfaces => {
                    PathRequestAudience::Network
                }
                RelayAudience::BoundaryAndGateway => PathRequestAudience::BoundaryAndGateway,
                RelayAudience::LocalClients => PathRequestAudience::LocalClients,
            };
            if path_request_egress_eligible(descriptor, Some(source), path_audience) {
                if !matches!(audience, RelayAudience::LocalClients)
                    && self.egress_path_request_limits.should_egress_limit(
                        descriptor.id,
                        now,
                        descriptor.common.path_request_egress,
                    )
                {
                    #[cfg(feature = "runtime-metrics")]
                    self.path_request_relay_counts
                        .record(PathRequestRelayOutcome::RateLimited);
                    continue;
                }
                match audience {
                    RelayAudience::OnlineNetworkInterfaces | RelayAudience::BoundaryAndGateway => {
                        let mut record_egress = || {
                            self.egress_path_request_limits
                                .record_egress(descriptor.id, now);
                            #[cfg(feature = "runtime-metrics")]
                            self.path_request_relay_counts
                                .record(PathRequestRelayOutcome::Sent);
                        };
                        sink(EngineReaction::Directive(Directive::SendIfOnline {
                            target: descriptor.id,
                            bytes: &buf[..wire_bytes],
                            on_send: &mut record_egress,
                        }));
                    }
                    RelayAudience::AllNetworkInterfaces => {
                        self.egress_path_request_limits
                            .record_egress(descriptor.id, now);
                        #[cfg(feature = "runtime-metrics")]
                        self.path_request_relay_counts
                            .record(PathRequestRelayOutcome::Sent);
                        sink(EngineReaction::Directive(Directive::Send {
                            target: descriptor.id,
                            bytes: &buf[..wire_bytes],
                        }));
                    }
                    RelayAudience::LocalClients => {
                        #[cfg(feature = "runtime-metrics")]
                        self.path_request_relay_counts
                            .record(PathRequestRelayOutcome::Sent);
                        sink(EngineReaction::Directive(Directive::Send {
                            target: descriptor.id,
                            bytes: &buf[..wire_bytes],
                        }))
                    }
                }
            }
        }
    }

    pub(super) fn relay_announce_to_local_clients(
        &self,
        destination: DestinationHash,
        hops: u8,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let Some(via) = self.transport_id() else {
            return;
        };
        let Some(stored) = self.routing_table.stored_announce_for(&destination) else {
            return;
        };
        let mut buf = [0u8; BROADCAST_MTU];
        let relay = ReemitAnnounce {
            announce: stored.announce.clone(),
            emit_hops: hops,
            via,
            target: source,
            is_path_response: false,
        };
        let Ok(written) = relay.to_wire(&mut buf) else {
            return;
        };
        for descriptor in interfaces {
            if descriptor.id == source
                || descriptor.id.kind() != Some(InterfaceKind::LocalClient)
                || !descriptor.capabilities.allows_transmit()
            {
                continue;
            }
            sink(EngineReaction::Directive(Directive::SendAnnounce {
                target: descriptor.id,
                bytes: &buf[..written],
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin: if source.kind() == Some(InterfaceKind::LocalClient) {
                    AnnounceOrigin::SharedClient
                } else {
                    AnnounceOrigin::Relay
                },
            }));
        }
    }
}
