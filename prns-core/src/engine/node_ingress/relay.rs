#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, PathRequestIdBytes, ReemitAnnounce,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceKind};
use crate::routing::path_requests::write_path_request_wire_packet;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, BROADCAST_MTU};

#[derive(Clone, Copy)]
pub(super) enum RelayAudience {
    Transports,
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
            let in_audience = match audience {
                RelayAudience::Transports => true,
                RelayAudience::LocalClients => {
                    descriptor.id.kind() == Some(InterfaceKind::LocalClient)
                }
            };
            if in_audience && descriptor.id != source && descriptor.capabilities.allows_transmit() {
                if matches!(audience, RelayAudience::Transports)
                    && self.egress_path_request_limits.should_egress_limit(
                        descriptor.id,
                        now,
                        descriptor.common.path_request_egress,
                    )
                {
                    continue;
                }
                if matches!(audience, RelayAudience::Transports) {
                    self.egress_path_request_limits
                        .record_egress(descriptor.id, now);
                }
                sink(EngineReaction::Directive(Directive::Send {
                    target: descriptor.id,
                    bytes: &buf[..wire_bytes],
                }));
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
