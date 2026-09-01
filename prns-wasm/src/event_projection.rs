use personal_rns::engine::{Journaled, LinkClosedReason, RouteRemovalCause};
use personal_rns::routing::delivery::Delivery;
use prns_host::{
    ApplicationEventKind, DiagnosticEventKind, EventField, EventProjection,
    EventProjectionExtensionField, EventProjectionField,
};

use crate::command_settlement::CapturedCommandSettlement;

pub(crate) enum CapturedJournal {
    Event(EventProjection),
    Control(CapturedCommandSettlement),
}

pub(crate) fn capture_journaled(journaled: Journaled<'_>) -> CapturedJournal {
    match journaled {
        Journaled::CommandSettled { id, settlement } => {
            CapturedJournal::Control(CapturedCommandSettlement::capture(id, settlement))
        }
        Journaled::PersistenceFlushed { cause, target } => CapturedJournal::Event(project(
            DiagnosticEventKind::PersistenceFlushed,
            [
                text(EventField::PersistenceCause, cause.name()),
                text(EventField::PersistenceTarget, target.name()),
            ],
        )),
        Journaled::PersistenceFlushFailed { cause, target } => CapturedJournal::Event(project(
            DiagnosticEventKind::PersistenceFlushFailed,
            [
                text(EventField::PersistenceCause, cause.name()),
                text(EventField::PersistenceTarget, target.name()),
            ],
        )),
        Journaled::AnnounceHeard { observation, .. } => CapturedJournal::Event(project(
            DiagnosticEventKind::AnnounceHeard,
            [
                bytes(EventField::AppData, observation.app_data),
                bytes(EventField::Destination, observation.destination.as_bytes()),
                u64(EventField::Hops, u64::from(observation.hops.0)),
                bytes(
                    EventField::SourceInterface,
                    observation.source_interface.as_bytes(),
                ),
            ],
        )),
        Journaled::SelfRatchetRotated { destination } => CapturedJournal::Event(project(
            DiagnosticEventKind::SelfRatchetRotated,
            [bytes(EventField::Destination, destination.as_bytes())],
        )),
        Journaled::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => CapturedJournal::Event(project(
            DiagnosticEventKind::AnnounceHeldDropped,
            [
                bytes(EventField::Destination, destination.as_bytes()),
                bytes(EventField::SourceInterface, source_interface.as_bytes()),
                text(EventField::Cause, &format!("{cause:?}")),
            ],
        )),
        Journaled::RemoteControlPairingExpired { endpoint } => {
            remote_control_diagnostic("RemoteControlPairingExpired", format!("{endpoint:?}"))
        }
        Journaled::RemoteControlPairingExpiryFailed { endpoint, failure } => {
            remote_control_diagnostic(
                "RemoteControlPairingExpiryFailed",
                format!("endpoint={endpoint:?}, failure={failure:?}"),
            )
        }
        Journaled::RemoteControlPairingAvailabilityObserved(observation) => {
            remote_control_diagnostic(
                "RemoteControlPairingAvailabilityObserved",
                format!("{observation:?}"),
            )
        }
        Journaled::RemoteControlTargetPairingConfirmationRequired(attempt) => {
            remote_control_diagnostic(
                "RemoteControlTargetPairingConfirmationRequired",
                format!("{attempt:?}"),
            )
        }
        Journaled::RemoteControlTargetPairingControllerCommitted { attempt_id } => {
            remote_control_diagnostic(
                "RemoteControlTargetPairingControllerCommitted",
                format!("{attempt_id:?}"),
            )
        }
        Journaled::RemoteControlTargetPairingAuthorizationRequired { attempt_id, grant } => {
            remote_control_diagnostic(
                "RemoteControlTargetPairingAuthorizationRequired",
                format!("attempt_id={attempt_id:?}, grant={grant:?}"),
            )
        }
        Journaled::RemoteControlControllerPairingConfirmationRequired(attempt) => {
            remote_control_diagnostic(
                "RemoteControlControllerPairingConfirmationRequired",
                format!("{attempt:?}"),
            )
        }
        Journaled::RemoteControlControllerPairingPersistenceRequired(persistence) => {
            remote_control_diagnostic(
                "RemoteControlControllerPairingPersistenceRequired",
                format!("{persistence:?}"),
            )
        }
        Journaled::RemoteControlControllerPairingExpired { aborted } => remote_control_diagnostic(
            "RemoteControlControllerPairingExpired",
            format!("{aborted:?}"),
        ),
        Journaled::RemoteControlControllerPairingLinkClosed { aborted } => {
            remote_control_diagnostic(
                "RemoteControlControllerPairingLinkClosed",
                format!("{aborted:?}"),
            )
        }
        Journaled::RemoteControlTargetPairingExpired { aborted } => {
            remote_control_diagnostic("RemoteControlTargetPairingExpired", format!("{aborted:?}"))
        }
        Journaled::RemoteControlTargetPairingLinkClosed { aborted } => remote_control_diagnostic(
            "RemoteControlTargetPairingLinkClosed",
            format!("{aborted:?}"),
        ),
        Journaled::RemoteControlTargetPairingCompletionRetentionExpired { attempt_id } => {
            remote_control_diagnostic(
                "RemoteControlTargetPairingCompletionRetentionExpired",
                format!("{attempt_id:?}"),
            )
        }
        Journaled::RemoteControlTargetPairingCompletionLinkClosed { attempt_id } => {
            remote_control_diagnostic(
                "RemoteControlTargetPairingCompletionLinkClosed",
                format!("{attempt_id:?}"),
            )
        }
        Journaled::LinkEstablished(link) => CapturedJournal::Event(project(
            DiagnosticEventKind::LinkEstablished,
            [
                bytes(EventField::LinkId, link.link_id.as_bytes()),
                u64(EventField::RttMillis, link.rtt_millis),
            ],
        )),
        Journaled::PeerIdentified { link_id, identity } => CapturedJournal::Event(project(
            DiagnosticEventKind::PeerIdentified,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                bytes(EventField::Identity, identity.as_bytes()),
            ],
        )),
        Journaled::RequestReceived {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            rtt,
            data,
            ..
        } => {
            let mut event = EventProjection::new(ApplicationEventKind::Request.into());
            event.set(bytes(EventField::Destination, destination.as_bytes()));
            event.set(bytes(EventField::LinkId, link_id.as_bytes()));
            event.set(bytes(EventField::RequestId, &request_id.0));
            if let Some(requester) = requester {
                event.set(bytes(EventField::Requester, requester.as_bytes()));
            }
            event.set(bytes(EventField::PathHash, path_hash.as_bytes()));
            event.set(u64(EventField::RttMillis, rtt.millis()));
            event.set(bytes(EventField::Data, data));
            CapturedJournal::Event(event)
        }
        Journaled::ResponseReceived {
            command_id,
            link_id,
            request_id,
            data,
            ..
        } => {
            let mut event = EventProjection::new(ApplicationEventKind::Response.into());
            event.set(EventProjectionField::u64(
                EventProjectionExtensionField::CommandId.into(),
                command_id.0,
            ));
            event.set(bytes(EventField::LinkId, link_id.as_bytes()));
            event.set(bytes(EventField::RequestId, &request_id.0));
            event.set(bytes(EventField::Data, data));
            CapturedJournal::Event(event)
        }
        Journaled::ResponseSegmentReceived {
            command_id,
            link_id,
            request_id,
            segment_index,
            total_segments,
            data,
            ..
        } => {
            let mut event = EventProjection::new(ApplicationEventKind::ResponseSegment.into());
            event.set(EventProjectionField::u64(
                EventProjectionExtensionField::CommandId.into(),
                command_id.0,
            ));
            event.set(bytes(EventField::LinkId, link_id.as_bytes()));
            event.set(bytes(EventField::RequestId, &request_id.0));
            event.set(u64(EventField::SegmentIndex, segment_index));
            event.set(u64(EventField::TotalSegments, total_segments));
            event.set(bytes(EventField::Data, data));
            CapturedJournal::Event(event)
        }
        Journaled::ChannelMessageReceived {
            link_id,
            message_type,
            data,
        } => CapturedJournal::Event(project(
            ApplicationEventKind::ChannelMessage,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                u64(EventField::MessageType, u64::from(message_type.0)),
                bytes(EventField::Data, data),
            ],
        )),
        Journaled::Delivered(Delivery::Single(delivery)) => CapturedJournal::Event(project(
            ApplicationEventKind::SingleDelivery,
            [
                bytes(EventField::Destination, delivery.destination.as_bytes()),
                bytes(EventField::Plaintext, delivery.plaintext),
                bytes(
                    EventField::SourceInterface,
                    delivery.source_interface.as_bytes(),
                ),
            ],
        )),
        Journaled::Delivered(Delivery::Link(delivery)) => CapturedJournal::Event(project(
            ApplicationEventKind::LinkDelivery,
            [
                bytes(EventField::LinkId, delivery.link_id.as_bytes()),
                bytes(EventField::Plaintext, delivery.plaintext),
                bytes(
                    EventField::SourceInterface,
                    delivery.source_interface.as_bytes(),
                ),
            ],
        )),
        Journaled::Delivered(Delivery::Plain(delivery)) => CapturedJournal::Event(project(
            DiagnosticEventKind::Delivered,
            [text(EventField::Detail, &format!("{delivery:?}"))],
        )),
        Journaled::Delivered(Delivery::Group(delivery)) => CapturedJournal::Event(project(
            DiagnosticEventKind::Delivered,
            [text(EventField::Detail, &format!("{delivery:?}"))],
        )),
        Journaled::LinkClosed { link_id, reason } => CapturedJournal::Event(project(
            DiagnosticEventKind::LinkClosed,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                text(
                    EventField::Reason,
                    match reason {
                        LinkClosedReason::Timeout => "timeout",
                        LinkClosedReason::PeerClosed => "peerClosed",
                        LinkClosedReason::MalformedRtt => "malformedRtt",
                        LinkClosedReason::LocallyClosed => "locallyClosed",
                    },
                ),
            ],
        )),
        Journaled::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => CapturedJournal::Event(project(
            DiagnosticEventKind::LinkInterfaceMismatch,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                bytes(EventField::AttachedInterface, attached_interface.as_bytes()),
                bytes(EventField::ArrivedOn, arrived_on.as_bytes()),
            ],
        )),
        Journaled::ResourceReceived {
            link_id,
            hash,
            metadata,
            data,
        } => {
            let mut event = EventProjection::new(ApplicationEventKind::ResourceAvailable.into());
            event.set(bytes(EventField::LinkId, link_id.as_bytes()));
            event.set(bytes(EventField::Hash, hash.as_bytes()));
            if let Some(metadata) = metadata {
                event.set(bytes(EventField::Metadata, metadata));
            }
            event.set(bytes(EventField::Data, data));
            CapturedJournal::Event(event)
        }
        Journaled::ResourceFailed {
            link_id,
            hash,
            cause,
        } => CapturedJournal::Event(project(
            DiagnosticEventKind::ResourceFailed,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                bytes(EventField::Hash, hash.as_bytes()),
                text(EventField::Cause, &format!("{cause:?}")),
            ],
        )),
        Journaled::ResourceSegmentReceived {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => {
            let mut event = EventProjection::new(ApplicationEventKind::ResourceSegment.into());
            event.set(bytes(EventField::LinkId, link_id.as_bytes()));
            event.set(bytes(EventField::OriginalHash, original_hash.as_bytes()));
            event.set(u64(EventField::SegmentIndex, segment_index));
            event.set(u64(EventField::TotalSegments, total_segments));
            if let Some(metadata) = metadata {
                event.set(bytes(EventField::Metadata, metadata));
            }
            event.set(bytes(EventField::Data, data));
            CapturedJournal::Event(event)
        }
        Journaled::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => CapturedJournal::Event(project(
            DiagnosticEventKind::ResourceAssembled,
            [
                bytes(EventField::LinkId, link_id.as_bytes()),
                bytes(EventField::OriginalHash, original_hash.as_bytes()),
                u64(EventField::TotalSizeBytes, total_size_bytes),
            ],
        )),
        Journaled::RouteRemoved { destination, cause } => {
            let kind = match cause {
                RouteRemovalCause::Expired => DiagnosticEventKind::RouteExpired,
                RouteRemovalCause::Evicted => DiagnosticEventKind::RouteEvicted,
                RouteRemovalCause::InterfaceGone => DiagnosticEventKind::RouteInterfaceGone,
                RouteRemovalCause::Dropped => DiagnosticEventKind::RouteDropped,
            };
            CapturedJournal::Event(project(
                kind,
                [bytes(EventField::Destination, destination.as_bytes())],
            ))
        }
    }
}

fn remote_control_diagnostic(kind: &str, detail: String) -> CapturedJournal {
    CapturedJournal::Event(project(
        DiagnosticEventKind::BackendDiagnostic,
        [
            text(EventField::Kind, kind),
            text(EventField::Detail, &detail),
        ],
    ))
}

fn project<const FIELD_COUNT: usize>(
    kind: impl Into<prns_host::EventProjectionKind>,
    fields: [EventProjectionField; FIELD_COUNT],
) -> EventProjection {
    let mut event = EventProjection::new(kind.into());
    for field in fields {
        event.set(field);
    }
    event
}

fn bytes(field: EventField, value: &[u8]) -> EventProjectionField {
    EventProjectionField::bytes(field.into(), value.to_vec())
}

fn text(field: EventField, value: &str) -> EventProjectionField {
    EventProjectionField::text(field.into(), value.to_string())
}

fn u64(field: EventField, value: u64) -> EventProjectionField {
    EventProjectionField::u64(field.into(), value)
}

#[cfg(test)]
mod tests {
    use personal_rns::engine::CommandId;
    use personal_rns::identity::IdentityHash;
    use personal_rns::remote_control::RemoteControlPairingIdentity;
    use personal_rns::routing::links::request::RequestId;
    use personal_rns::routing::links::LinkId;
    use prns_host::{ApplicationEventKind, DiagnosticEventKind};

    use super::*;

    #[test]
    fn response_projection_preserves_command_correlation_and_payload() {
        let captured = capture_journaled(Journaled::ResponseReceived {
            command_id: CommandId(7),
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            data: &[3, 4, 5],
        });
        let CapturedJournal::Event(event) = captured else {
            panic!("response projected as control data");
        };

        assert_eq!(event.kind(), ApplicationEventKind::Response.into());
        assert_eq!(
            event.fields(),
            &[
                EventProjectionField::u64(EventProjectionExtensionField::CommandId.into(), 7,),
                EventProjectionField::bytes(EventField::LinkId.into(), vec![1; 16]),
                EventProjectionField::bytes(EventField::RequestId.into(), vec![2; 16]),
                EventProjectionField::bytes(EventField::Data.into(), vec![3, 4, 5]),
            ]
        );
    }

    #[test]
    fn remote_control_journals_remain_visible_without_a_public_host_variant() {
        let endpoint = RemoteControlPairingIdentity::new(IdentityHash::new([0x81; 16])).endpoint();
        let captured = capture_journaled(Journaled::RemoteControlPairingExpired { endpoint });
        let CapturedJournal::Event(event) = captured else {
            panic!("remote-control journal projected as control data");
        };

        assert_eq!(event.kind(), DiagnosticEventKind::BackendDiagnostic.into());
        assert_eq!(
            event.fields(),
            &[
                EventProjectionField::text(
                    EventField::Kind.into(),
                    "RemoteControlPairingExpired".to_string(),
                ),
                EventProjectionField::text(EventField::Detail.into(), format!("{endpoint:?}"),),
            ]
        );
    }
}
