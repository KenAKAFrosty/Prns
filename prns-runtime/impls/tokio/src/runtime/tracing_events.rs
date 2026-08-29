use crate::engine::Settlement;
use crate::routing::delivery::Delivery;

use super::{Diagnostic, Message, PrnsEvent};

pub(crate) fn emit(event: &PrnsEvent<'_>) {
    match event {
        PrnsEvent::Message(message) => {
            if tracing::enabled!(target: "prns.runtime", tracing::Level::DEBUG) {
                emit_message(message);
            }
        }
        PrnsEvent::Diagnostic(diagnostic) => emit_diagnostic(diagnostic),
    }
}

fn emit_message(message: &Message<'_>) {
    match message {
        Message::Delivered(delivery) => {
            let (kind, bytes, interface_kind) = delivery_fields(delivery);
            tracing::debug!(
                target: "prns.runtime",
                event = "message_delivered",
                message_kind = kind,
                bytes,
                interface_kind = ?interface_kind,
                delivery = ?delivery_identity(delivery),
            );
        }
        Message::Request {
            link_id,
            request_id,
            requester: _,
            path_hash,
            rtt,
            data,
            ..
        } => tracing::debug!(
            target: "prns.runtime",
            event = "request_received",
            bytes = data.len(),
            rtt_millis = rtt.millis(),
            link_id = ?link_id.as_bytes(),
            request_id = ?request_id,
            path_hash = ?path_hash,
        ),
        Message::Response {
            link_id,
            request_id,
            data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "response_received",
            bytes = data.len(),
            link_id = ?link_id.as_bytes(),
            request_id = ?request_id,
        ),
        Message::ResponseSegment {
            link_id,
            request_id,
            segment_index,
            total_segments,
            data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "response_segment_received",
            bytes = data.len(),
            segment_index,
            total_segments,
            link_id = ?link_id.as_bytes(),
            request_id = ?request_id,
        ),
        Message::Resource {
            link_id,
            hash,
            metadata,
            data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "resource_received",
            bytes = data.len(),
            metadata_bytes = metadata.map_or(0, <[u8]>::len),
            link_id = ?link_id.as_bytes(),
            resource_hash = ?hash.as_bytes(),
        ),
        Message::ResourceNeedsDecompression {
            link_id,
            hash,
            stream,
            uncompressed_data_bytes,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "resource_decompression_requested",
            compressed_bytes = stream.len(),
            uncompressed_bytes = uncompressed_data_bytes,
            link_id = ?link_id.as_bytes(),
            resource_hash = ?hash.as_bytes(),
        ),
        Message::ResourceSegment {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "resource_segment_received",
            bytes = data.len(),
            metadata_bytes = metadata.map_or(0, <[u8]>::len),
            segment_index,
            total_segments,
            link_id = ?link_id.as_bytes(),
            resource_hash = ?original_hash.as_bytes(),
        ),
        Message::ChannelMessage {
            link_id,
            message_type,
            data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "channel_message_received",
            bytes = data.len(),
            message_type = ?message_type,
            link_id = ?link_id.as_bytes(),
        ),
    }
}

fn emit_diagnostic(diagnostic: &Diagnostic<'_>) {
    match diagnostic {
        Diagnostic::PersistenceRestored {
            routes,
            destination_identities,
            tunnels,
            ratchets,
            refused,
            dropped,
        } => {
            tracing::info!(
                target: "prns.runtime",
                event = "persistence_restored",
                routes,
                destination_identities,
                tunnels,
                ratchets,
                refused,
                dropped,
            );
        }
        Diagnostic::PersistenceFlushed { cause, target } => {
            tracing::debug!(
                target: "prns.runtime",
                event = "persistence_flushed",
                cause = cause.name(),
                persistence_target = target.name(),
            );
        }
        Diagnostic::PersistenceFlushFailed { cause, target } => {
            tracing::error!(
                target: "prns.runtime",
                event = "persistence_flush_failed",
                cause = cause.name(),
                persistence_target = target.name(),
            );
        }
        Diagnostic::SelfRatchetRotated { destination } => {
            tracing::info!(target: "prns.runtime", event = "self_ratchet_rotated");
            tracing::debug!(
                target: "prns.runtime",
                event = "self_ratchet_rotated_detail",
                destination = ?destination.as_bytes(),
            );
        }
        Diagnostic::AnnounceHeard {
            destination,
            hops,
            source_interface,
            app_data,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "announce_heard",
            hops,
            app_data_bytes = app_data.len(),
            interface_kind = ?source_interface.kind(),
            destination = ?destination.as_bytes(),
            interface_id = ?source_interface.as_bytes(),
        ),
        Diagnostic::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "announce_held_dropped",
            cause = ?cause,
            interface_kind = ?source_interface.kind(),
            destination = ?destination.as_bytes(),
            interface_id = ?source_interface.as_bytes(),
        ),
        Diagnostic::CommandSettled { id, settlement } => tracing::debug!(
            target: "prns.runtime",
            event = "command_settled",
            command = settlement_kind(settlement),
            outcome = settlement_outcome(settlement),
            command_id = id.0,
        ),
        Diagnostic::LinkEstablished(established) => {
            tracing::info!(
                target: "prns.runtime",
                event = "link_established",
                rtt_millis = established.rtt_millis,
            );
            tracing::debug!(
                target: "prns.runtime",
                event = "link_established_detail",
                link_id = ?established.link_id.as_bytes(),
            );
        }
        Diagnostic::PeerIdentified { link_id, identity } => {
            tracing::info!(target: "prns.runtime", event = "peer_identified");
            tracing::debug!(
                target: "prns.runtime",
                event = "peer_identified_detail",
                link_id = ?link_id.as_bytes(),
                identity = ?identity.as_bytes(),
            );
        }
        Diagnostic::LinkClosed { link_id, reason } => {
            tracing::info!(
                target: "prns.runtime",
                event = "link_closed",
                reason = ?reason,
            );
            tracing::debug!(
                target: "prns.runtime",
                event = "link_closed_detail",
                link_id = ?link_id.as_bytes(),
            );
        }
        Diagnostic::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            tracing::warn!(
                target: "prns.runtime",
                event = "link_interface_mismatch",
                attached_kind = ?attached_interface.kind(),
                arrived_kind = ?arrived_on.kind(),
            );
            tracing::debug!(
                target: "prns.runtime",
                event = "link_interface_mismatch_detail",
                link_id = ?link_id.as_bytes(),
                attached_interface = ?attached_interface.as_bytes(),
                arrived_on = ?arrived_on.as_bytes(),
            );
        }
        Diagnostic::ResourceFailed {
            link_id,
            hash,
            cause,
        } => {
            tracing::warn!(
                target: "prns.runtime",
                event = "resource_failed",
                cause = ?cause,
            );
            tracing::debug!(
                target: "prns.runtime",
                event = "resource_failed_detail",
                link_id = ?link_id.as_bytes(),
                resource_hash = ?hash.as_bytes(),
            );
        }
        Diagnostic::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => tracing::debug!(
            target: "prns.runtime",
            event = "resource_assembled",
            bytes = total_size_bytes,
            link_id = ?link_id.as_bytes(),
            resource_hash = ?original_hash.as_bytes(),
        ),
        Diagnostic::RouteRemoved { destination, cause } => tracing::debug!(
            target: "prns.runtime",
            event = "route_removed",
            cause = ?cause,
            destination = ?destination.as_bytes(),
        ),
    }
}

fn delivery_fields(
    delivery: &Delivery<'_>,
) -> (
    &'static str,
    usize,
    Option<crate::interfaces::InterfaceKind>,
) {
    match delivery {
        Delivery::Plain(delivery) => (
            "plain",
            delivery.payload.len(),
            delivery.source_interface.kind(),
        ),
        Delivery::Single(delivery) => (
            "single",
            delivery.plaintext.len(),
            delivery.source_interface.kind(),
        ),
        Delivery::Group(delivery) => (
            "group",
            delivery.plaintext.len(),
            delivery.source_interface.kind(),
        ),
        Delivery::Link(delivery) => (
            "link",
            delivery.plaintext.len(),
            delivery.source_interface.kind(),
        ),
    }
}

fn delivery_identity<'a>(delivery: &'a Delivery<'_>) -> &'a [u8] {
    match delivery {
        Delivery::Plain(delivery) => delivery.destination.as_bytes(),
        Delivery::Single(delivery) => delivery.destination.as_bytes(),
        Delivery::Group(delivery) => delivery.destination.as_bytes(),
        Delivery::Link(delivery) => delivery.link_id.as_bytes(),
    }
}

fn settlement_kind(settlement: &Settlement) -> &'static str {
    match settlement {
        Settlement::AnnounceNow(_) => "announce_now",
        Settlement::SetRegisteredAnnounceAppData(_) => "set_registered_announce_app_data",
        Settlement::SendSinglePacket(_) => "send_single_packet",
        Settlement::SendGroup(_) => "send_group",
        Settlement::SendPlainPacket(_) => "send_plain_packet",
        Settlement::RequestPath(_) => "request_path",
        Settlement::EstablishLink(_) => "establish_link",
        Settlement::SendToLink(_) => "send_to_link",
        Settlement::Identify(_) => "identify",
        Settlement::SendRequest(_) => "send_request",
        Settlement::Respond(_) => "respond",
        Settlement::CloseLink(_) => "close_link",
        Settlement::SendResource(_) => "send_resource",
        Settlement::SetResourceStrategy(_) => "set_resource_strategy",
        Settlement::SendToChannel(_) => "send_to_channel",
        Settlement::AllowRequester(_) => "allow_requester",
    }
}

fn settlement_outcome(settlement: &Settlement) -> &'static str {
    let succeeded = match settlement {
        Settlement::AnnounceNow(result) => result.is_ok(),
        Settlement::SetRegisteredAnnounceAppData(result) => result.is_ok(),
        Settlement::SendSinglePacket(result) => result.is_ok(),
        Settlement::SendGroup(result) => result.is_ok(),
        Settlement::SendPlainPacket(result) => result.is_ok(),
        Settlement::RequestPath(result) => result.is_ok(),
        Settlement::EstablishLink(result) => result.is_ok(),
        Settlement::SendToLink(result) => result.is_ok(),
        Settlement::Identify(result) => result.is_ok(),
        Settlement::SendRequest(result) => result.is_ok(),
        Settlement::Respond(result) => result.is_ok(),
        Settlement::CloseLink(result) => result.is_ok(),
        Settlement::SendResource(result) => result.is_ok(),
        Settlement::SetResourceStrategy(result) => result.is_ok(),
        Settlement::SendToChannel(result) => result.is_ok(),
        Settlement::AllowRequester(result) => result.is_ok(),
    };
    if succeeded {
        "succeeded"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};

    use super::*;
    use crate::wire::DestinationHash;

    #[derive(Debug)]
    struct CapturedEvent {
        level: Level,
        target: &'static str,
        fields: HashMap<&'static str, String>,
    }

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<CapturedEvent>>>);

    struct FieldVisitor<'a>(&'a mut HashMap<&'static str, String>);

    impl Visit for FieldVisitor<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name(), value.to_string());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name(), format!("{value:?}"));
        }
    }

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut fields = HashMap::new();
            event.record(&mut FieldVisitor(&mut fields));
            self.0.lock().unwrap().push(CapturedEvent {
                level: *event.metadata().level(),
                target: event.metadata().target(),
                fields,
            });
        }
    }

    #[test]
    fn info_lifecycle_events_keep_identifiers_in_debug_detail() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Capture(captured.clone()));
        tracing::subscriber::with_default(subscriber, || {
            emit(&PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated {
                destination: DestinationHash::new([0xA5; 16]),
            }));
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].level, Level::INFO);
        assert_eq!(events[0].target, "prns.runtime");
        assert_eq!(
            events[0].fields.get("event").map(String::as_str),
            Some("self_ratchet_rotated")
        );
        assert!(!events[0].fields.contains_key("destination"));
        assert_eq!(events[1].level, Level::DEBUG);
        assert_eq!(
            events[1].fields.get("event").map(String::as_str),
            Some("self_ratchet_rotated_detail")
        );
        assert!(events[1].fields.contains_key("destination"));
    }

    #[test]
    fn announce_trace_records_application_data_size_without_its_bytes() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Capture(captured.clone()));
        tracing::subscriber::with_default(subscriber, || {
            emit(&PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination: DestinationHash::new([0xA5; 16]),
                hops: 2,
                source_interface: crate::interfaces::InterfaceId::new([0x5A; 8]),
                app_data: &[0x00, 0x70, 0x72, 0x6E, 0x73, 0xFF],
            }));
        });

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].fields.get("app_data_bytes").map(String::as_str),
            Some("6")
        );
        assert!(!events[0].fields.contains_key("app_data"));
    }
}
