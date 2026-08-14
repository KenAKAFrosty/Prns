use js_sys::{Array, BigInt, Object, Reflect, Uint8Array};
use personal_rns::engine::{
    AllowRequesterFailure, AllowRequesterRejection, AnnounceNowFailure, AnnounceNowRejection,
    CloseLinkFailure, CloseLinkRejection, DeliveryEvidence, DeliveryProof, EstablishLinkFailure,
    EstablishLinkRejection, FanTarget, IdentifyFailure, IdentifyRejection, Journaled,
    LinkClosedReason, RequestPathFailure, RespondFailure, RespondRejection, RouteRemovalCause,
    SendRequestFailure, SendRequestRejection, SendResourceFailure, SendResourceRejection,
    SendSinglePacketFailure, SendSinglePacketRejection, SendToChannelFailure,
    SendToChannelRejection, SendToLinkFailure, SendToLinkRejection, SetResourceStrategyFailure,
    SetResourceStrategyRejection, Settlement,
};
use personal_rns::interfaces::bluetooth_auto as bluetooth_contract;
use personal_rns::interfaces::lora::{RadioProfile, PROFILE_WIRE_LEN};
use personal_rns::interfaces::usb_auto;
use personal_rns::interfaces::{ConnectionState, InterfaceKind};
use personal_rns::routing::delivery::Delivery;
use wasm_bindgen::prelude::*;

use crate::runtime::{OutboundFrame, OutboundTarget};

pub(crate) fn journaled_to_js(journaled: Journaled<'_>) -> JsValue {
    let object = Object::new();
    match journaled {
        Journaled::PersistenceFlushed { cause, target } => {
            set_str(&object, "type", "persistenceFlushed");
            set_str(&object, "cause", cause.name());
            set_str(&object, "target", target.name());
        }
        Journaled::PersistenceFlushFailed { cause, target } => {
            set_str(&object, "type", "persistenceFlushFailed");
            set_str(&object, "cause", cause.name());
            set_str(&object, "target", target.name());
        }
        Journaled::AnnounceHeard { observation, .. } => {
            set_str(&object, "type", "announce");
            set_bytes(&object, "destination", observation.destination.as_bytes());
            set_u32(&object, "hops", u32::from(observation.hops.0));
            set_bytes(
                &object,
                "sourceInterface",
                observation.source_interface.as_bytes(),
            );
        }
        Journaled::SelfRatchetRotated { destination } => {
            set_str(&object, "type", "selfRatchetRotated");
            set_bytes(&object, "destination", destination.as_bytes());
        }
        Journaled::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => {
            set_str(&object, "type", "announceHeldDropped");
            set_bytes(&object, "destination", destination.as_bytes());
            set_bytes(&object, "sourceInterface", source_interface.as_bytes());
            set_str(&object, "cause", &format!("{cause:?}"));
        }
        Journaled::CommandSettled { id, settlement } => {
            set_str(&object, "type", "commandSettled");
            set_u64(&object, "id", id.0);
            settlement_to_js(&object, settlement);
        }
        Journaled::LinkEstablished(link) => {
            set_str(&object, "type", "linkEstablished");
            set_bytes(&object, "linkId", link.link_id.as_bytes());
            set_u64(&object, "rttMillis", link.rtt_millis);
        }
        Journaled::PeerIdentified { link_id, identity } => {
            set_str(&object, "type", "peerIdentified");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "identity", identity.as_bytes());
        }
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
            set_str(&object, "type", "request");
            set_bytes(&object, "destination", destination.as_bytes());
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "requestId", &request_id.0);
            if let Some(requester) = requester {
                set_bytes(&object, "requester", requester.as_bytes());
            }
            set_bytes(&object, "pathHash", path_hash.as_bytes());
            set_u64(&object, "rttMillis", rtt.millis());
            set_bytes(&object, "data", data);
        }
        Journaled::ResponseReceived {
            command_id,
            link_id,
            request_id,
            data,
            ..
        } => {
            set_str(&object, "type", "response");
            set_u64(&object, "commandId", command_id.0);
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "requestId", &request_id.0);
            set_bytes(&object, "data", data);
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
            set_str(&object, "type", "responseSegment");
            set_u64(&object, "commandId", command_id.0);
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "requestId", &request_id.0);
            set_u64(&object, "segmentIndex", segment_index);
            set_u64(&object, "totalSegments", total_segments);
            set_bytes(&object, "data", data);
        }
        Journaled::ChannelMessageReceived {
            link_id,
            message_type,
            data,
        } => {
            set_str(&object, "type", "channelMessage");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_u64(&object, "messageType", u64::from(message_type.0));
            set_bytes(&object, "data", data);
        }
        Journaled::Delivered(Delivery::Single(delivery)) => {
            set_str(&object, "type", "singleDelivery");
            set_bytes(&object, "destination", delivery.destination.as_bytes());
            set_bytes(&object, "plaintext", delivery.plaintext);
            set_bytes(
                &object,
                "sourceInterface",
                delivery.source_interface.as_bytes(),
            );
        }
        Journaled::Delivered(Delivery::Plain(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::Delivered(Delivery::Group(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::Delivered(Delivery::Link(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::LinkClosed { link_id, reason } => {
            set_str(&object, "type", "linkClosed");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_str(
                &object,
                "reason",
                match reason {
                    LinkClosedReason::Timeout => "timeout",
                    LinkClosedReason::PeerClosed => "peerClosed",
                    LinkClosedReason::MalformedRtt => "malformedRtt",
                },
            );
        }
        Journaled::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            set_str(&object, "type", "linkInterfaceMismatch");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "attachedInterface", attached_interface.as_bytes());
            set_bytes(&object, "arrivedOn", arrived_on.as_bytes());
        }
        Journaled::ResourceReceived {
            link_id,
            hash,
            metadata,
            data,
        } => {
            set_str(&object, "type", "resourceReceived");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "hash", hash.as_bytes());
            if let Some(metadata) = metadata {
                set_bytes(&object, "metadata", metadata);
            }
            set_bytes(&object, "data", data);
        }
        Journaled::ResourceFailed {
            link_id,
            hash,
            cause,
        } => {
            set_str(&object, "type", "resourceFailed");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "hash", hash.as_bytes());
            set_str(&object, "cause", &format!("{cause:?}"));
        }
        Journaled::ResourceNeedsDecompression {
            link_id,
            hash,
            stream,
            uncompressed_data_bytes,
        } => {
            set_str(&object, "type", "resourceNeedsDecompression");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "hash", hash.as_bytes());
            set_bytes(&object, "stream", stream);
            set_bigint(&object, "uncompressedDataBytes", uncompressed_data_bytes);
        }
        Journaled::ResourceSegmentReceived {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => {
            set_str(&object, "type", "resourceSegment");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "originalHash", original_hash.as_bytes());
            set_u64(&object, "segmentIndex", segment_index);
            set_u64(&object, "totalSegments", total_segments);
            if let Some(metadata) = metadata {
                set_bytes(&object, "metadata", metadata);
            }
            set_bytes(&object, "data", data);
        }
        Journaled::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => {
            set_str(&object, "type", "resourceAssembled");
            set_bytes(&object, "linkId", link_id.as_bytes());
            set_bytes(&object, "originalHash", original_hash.as_bytes());
            set_bigint(&object, "totalSizeBytes", total_size_bytes);
        }
        Journaled::RouteRemoved { destination, cause } => {
            let kind = match cause {
                RouteRemovalCause::Expired => "routeExpired",
                RouteRemovalCause::Evicted => "routeEvicted",
                RouteRemovalCause::InterfaceGone => "routeInterfaceGone",
                RouteRemovalCause::Dropped => "routeDropped",
            };
            set_str(&object, "type", kind);
            set_bytes(&object, "destination", destination.as_bytes());
        }
    }
    object.into()
}

fn settlement_to_js(object: &Object, settlement: Settlement) {
    match settlement {
        Settlement::AnnounceNow(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "Announced");
        }
        Settlement::AnnounceNow(Err(failure)) => match failure {
            AnnounceNowFailure::Rejected(AnnounceNowRejection::UnknownDestination) => {
                set_command_failure(object, "UnknownDestination", None);
            }
            AnnounceNowFailure::Rejected(AnnounceNowRejection::NotASingleDestination) => {
                set_command_failure(object, "NotSingleDestination", None);
            }
            AnnounceNowFailure::Rejected(AnnounceNowRejection::AppDataTooLong) => {
                set_command_failure(object, "AnnounceAppDataTooLong", None);
            }
            AnnounceNowFailure::Rejected(AnnounceNowRejection::UnknownInterface) => {
                set_command_failure(object, "UnknownInterface", None);
            }
            AnnounceNowFailure::WriteFailed(error) => {
                set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
            }
        },
        Settlement::SendSinglePacket(Ok(delivered)) => {
            set_packet_delivered(object, delivered);
        }
        Settlement::SendSinglePacket(Err(failure)) => match failure {
            SendSinglePacketFailure::Rejected(SendSinglePacketRejection::NoRouteToDestination) => {
                set_command_failure(object, "NoRouteToDestination", None);
            }
            SendSinglePacketFailure::Rejected(SendSinglePacketRejection::NotDirectlyReachable) => {
                set_command_failure(object, "NotDirectlyReachable", None);
            }
            SendSinglePacketFailure::WriteFailed(error) => {
                set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
            }
            SendSinglePacketFailure::Culled => {
                set_command_failure(object, "PacketCulled", None);
            }
            SendSinglePacketFailure::Timeout => {
                set_command_failure(object, "DeliveryTimedOut", None);
            }
        },
        Settlement::CloseLink(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "LinkCloseQueued");
        }
        Settlement::CloseLink(Err(CloseLinkFailure::Rejected(CloseLinkRejection::NoSuchLink))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::CloseLink(Err(CloseLinkFailure::Rejected(
            CloseLinkRejection::LinkNotActive,
        ))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::CloseLink(Err(CloseLinkFailure::WriteFailed)) => {
            set_command_failure(object, "WriteFailed", Some("link write failed".to_string()));
        }
        Settlement::RequestPath(Ok(path)) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "PathDiscovered");
            set_u32(object, "hops", u32::from(path.hops.0));
        }
        Settlement::RequestPath(Err(RequestPathFailure::WriteFailed(error))) => {
            set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
        }
        Settlement::RequestPath(Err(RequestPathFailure::Timeout)) => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        Settlement::RequestPath(Err(RequestPathFailure::Culled)) => {
            set_command_failure(object, "PacketCulled", None);
        }
        Settlement::EstablishLink(Ok(link)) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "LinkEstablished");
            set_bytes(object, "linkId", link.link_id.as_bytes());
            set_u64(object, "rttMillis", link.rtt_millis);
        }
        Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination,
        ))) => {
            set_command_failure(object, "NoRouteToDestination", None);
        }
        Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NotDirectlyReachable,
        ))) => {
            set_command_failure(object, "NotDirectlyReachable", None);
        }
        Settlement::EstablishLink(Err(EstablishLinkFailure::WriteFailed(error))) => {
            set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
        }
        Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)) => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        Settlement::SendToLink(Ok(delivered)) => {
            set_packet_delivered(object, delivered);
        }
        Settlement::SendToLink(Err(SendToLinkFailure::Rejected(
            SendToLinkRejection::NoSuchLink,
        ))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::SendToLink(Err(SendToLinkFailure::Rejected(
            SendToLinkRejection::LinkNotActive,
        ))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::SendToLink(Err(SendToLinkFailure::WriteFailed(error))) => {
            set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
        }
        Settlement::SendToLink(Err(SendToLinkFailure::Culled)) => {
            set_command_failure(object, "PacketCulled", None);
        }
        Settlement::SendToLink(Err(SendToLinkFailure::Timeout)) => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        Settlement::Identify(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "Identified");
        }
        Settlement::Identify(Err(IdentifyFailure::Rejected(IdentifyRejection::NoSuchLink))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::Identify(Err(IdentifyFailure::Rejected(IdentifyRejection::LinkNotActive))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::Identify(Err(IdentifyFailure::Rejected(IdentifyRejection::NotInitiator))) => {
            set_command_failure(object, "NotLinkInitiator", None);
        }
        Settlement::Identify(Err(IdentifyFailure::Rejected(
            IdentifyRejection::IdentityNotHeld,
        ))) => {
            set_command_failure(object, "IdentityNotHeld", None);
        }
        Settlement::Identify(Err(IdentifyFailure::WriteFailed)) => {
            set_command_failure(
                object,
                "WriteFailed",
                Some("identity write failed".to_string()),
            );
        }
        Settlement::SendRequest(Ok(delivered)) => {
            set_packet_delivered(object, delivered);
        }
        Settlement::SendRequest(Err(SendRequestFailure::Rejected(
            SendRequestRejection::NoSuchLink,
        ))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::SendRequest(Err(SendRequestFailure::Rejected(
            SendRequestRejection::LinkNotActive,
        ))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::SendRequest(Err(SendRequestFailure::WriteFailed)) => {
            set_command_failure(
                object,
                "WriteFailed",
                Some("request write failed".to_string()),
            );
        }
        Settlement::SendRequest(Err(SendRequestFailure::Culled)) => {
            set_command_failure(object, "PacketCulled", None);
        }
        Settlement::SendRequest(Err(SendRequestFailure::Timeout)) => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)) => {
            set_command_failure(object, "ResponseTooLarge", None);
        }
        Settlement::Respond(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "ResponseSent");
            set_u64(object, "rttMillis", 0);
        }
        Settlement::Respond(Err(RespondFailure::Rejected(RespondRejection::NoSuchLink))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::Respond(Err(RespondFailure::Rejected(RespondRejection::LinkNotActive))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::Respond(Err(RespondFailure::WriteFailed)) => {
            set_command_failure(
                object,
                "WriteFailed",
                Some("response write failed".to_string()),
            );
        }
        Settlement::Respond(Err(RespondFailure::Resource(failure))) => {
            set_resource_failure(object, failure);
        }
        Settlement::SendResource(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "ResourceSent");
        }
        Settlement::SendResource(Err(failure)) => {
            set_resource_failure(object, failure);
        }
        Settlement::SetResourceStrategy(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "ResourceStrategySet");
        }
        Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
            SetResourceStrategyRejection::NoSuchLink,
        ))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
            SetResourceStrategyRejection::LinkNotActive,
        ))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::SendToChannel(Ok(delivered)) => {
            set_packet_delivered(object, delivered);
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::Rejected(
            SendToChannelRejection::NoSuchLink,
        ))) => {
            set_command_failure(object, "UnknownLink", None);
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::Rejected(
            SendToChannelRejection::LinkNotActive,
        ))) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::WriteFailed(error))) => {
            set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::WindowFull)) => {
            set_command_failure(object, "ChannelWindowFull", None);
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::Untrackable)) => {
            set_command_failure(object, "ChannelUntrackable", None);
        }
        Settlement::SendToChannel(Err(SendToChannelFailure::Timeout)) => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        Settlement::AllowRequester(Ok(())) => {
            set_str(object, "result", "succeeded");
            set_str(object, "kind", "RequesterAllowed");
        }
        Settlement::AllowRequester(Err(AllowRequesterFailure::Rejected(
            AllowRequesterRejection::NoSuchHandler,
        ))) => {
            set_command_failure(object, "UnknownRequestHandler", None);
        }
        Settlement::AllowRequester(Err(AllowRequesterFailure::Rejected(
            AllowRequesterRejection::NoAllowList,
        ))) => {
            set_command_failure(object, "RequestPolicyNotAllowList", None);
        }
        Settlement::AllowRequester(Err(AllowRequesterFailure::Rejected(
            AllowRequesterRejection::AllowListFull,
        ))) => {
            set_command_failure(object, "RequestAllowListFull", None);
        }
        Settlement::SendGroup(_) => {
            set_str(object, "result", "untracked");
        }
    }
}

fn set_packet_delivered(object: &Object, delivered: personal_rns::engine::PacketReceiptDelivered) {
    set_str(object, "result", "succeeded");
    set_str(object, "kind", "PacketDelivered");
    set_u64(object, "rttMillis", delivered.rtt.millis());
    match delivered.evidence {
        DeliveryEvidence::Proof(DeliveryProof::Explicit(hash)) => {
            set_str(object, "evidence", "ExplicitProof");
            set_bytes(object, "packetHash", hash.as_bytes());
        }
        DeliveryEvidence::Proof(DeliveryProof::Implicit(hash)) => {
            set_str(object, "evidence", "ImplicitProof");
            set_bytes(object, "packetHash", hash.as_bytes());
        }
        DeliveryEvidence::Response => {
            set_str(object, "evidence", "Response");
        }
    }
}

fn set_resource_failure(object: &Object, failure: SendResourceFailure) {
    match failure {
        SendResourceFailure::Rejected(SendResourceRejection::NoSuchLink) => {
            set_command_failure(object, "UnknownLink", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::LinkNotActive) => {
            set_command_failure(object, "LinkNotActive", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::LinkBusy) => {
            set_command_failure(object, "LinkBusy", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::TableFull) => {
            set_command_failure(object, "ResourceTableFull", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::Build(
            personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::DataTooLarge,
        )) => {
            set_command_failure(object, "PayloadTooLarge", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::Build(
            personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::MetadataTooLarge,
        ))
        | SendResourceFailure::Rejected(SendResourceRejection::MetadataMisplaced) => {
            set_command_failure(object, "ResourceMetadataTooLarge", None);
        }
        SendResourceFailure::Rejected(SendResourceRejection::Build(error)) => {
            set_command_failure(object, "WriteFailed", Some(format!("{error:?}")));
        }
        SendResourceFailure::WriteFailed => {
            set_command_failure(object, "WriteFailed", Some("resource write failed".to_string()));
        }
        SendResourceFailure::RejectedByPeer => {
            set_command_failure(object, "ResourceRejectedByPeer", None);
        }
        SendResourceFailure::Sequencing => {
            set_command_failure(object, "ResourceSequencingFailed", None);
        }
        SendResourceFailure::Timeout => {
            set_command_failure(object, "DeliveryTimedOut", None);
        }
        SendResourceFailure::PredecessorFailed => {
            set_command_failure(object, "ResourcePredecessorFailed", None);
        }
    }
}

fn set_command_failure(object: &Object, kind: &str, detail: Option<String>) {
    set_str(object, "result", "failed");
    set_str(object, "kind", kind);
    if let Some(detail) = detail {
        set_str(object, "detail", &detail);
    }
}

pub(crate) fn outbound_to_js(frame: &OutboundFrame) -> JsValue {
    let object = Object::new();
    set_str(
        &object,
        "type",
        if frame.announce { "announce" } else { "frame" },
    );
    set_value(
        &object,
        "target",
        outbound_target_to_js(frame.target.clone()),
    );
    if let Some(hops) = frame.hops {
        set_u32(&object, "hops", hops as u32);
    }
    set_bytes(&object, "bytes", &frame.bytes);
    object.into()
}

pub(crate) fn config_result_to_str(result: usb_auto::ConfigResult) -> &'static str {
    match result {
        usb_auto::ConfigResult::Ok => "ok",
        usb_auto::ConfigResult::ApplyFailed => "applyFailed",
        usb_auto::ConfigResult::ProfileNotSaved => "profileNotSaved",
        usb_auto::ConfigResult::Rejected => "rejected",
        usb_auto::ConfigResult::BadPayload => "badPayload",
    }
}

pub(crate) fn usb_auto_message_to_js(message: usb_auto::Message<'_>) -> JsValue {
    let object = Object::new();
    match message {
        usb_auto::Message::Hello(_) => set_str(&object, "type", "hello"),
        usb_auto::Message::HelloAck { tag, .. } => {
            set_str(&object, "type", "helloAck");
            set_bytes(&object, "tag", &tag.0);
        }
        usb_auto::Message::Data(packet) => {
            set_str(&object, "type", "data");
            set_bytes(&object, "bytes", packet);
        }
        usb_auto::Message::ConfigRequest { request_id, action } => {
            set_str(&object, "type", "configRequest");
            set_u32(&object, "requestId", request_id as u32);
            set_bytes(&object, "action", action);
        }
        usb_auto::Message::ConfigResponse {
            request_id,
            result,
            detail,
        } => {
            set_str(&object, "type", "configResponse");
            set_u32(&object, "requestId", request_id as u32);
            set_str(&object, "result", config_result_to_str(result));
            set_bytes(&object, "detail", detail);
        }
        usb_auto::Message::Snapshot {
            schema_version,
            body,
        } => {
            set_str(&object, "type", "snapshot");
            set_u32(&object, "schemaVersion", schema_version as u32);
            set_bytes(&object, "body", body);
        }
    }
    object.into()
}

/// Decode a snapshot body and project it onto a plain JS object the `/configure`
/// webUI renders. Returns `Err` when the body is malformed so the JS layer can
/// surface a decode failure rather than silently dropping the snapshot.
pub(crate) fn snapshot_body_to_js(body: &[u8]) -> Result<JsValue, usb_auto::SnapshotDecodeError> {
    let decoded = usb_auto::SnapshotBody::decode(body)?;
    let object = Object::new();
    let sections = Array::new();
    for section in &decoded.sections {
        sections.push(&snapshot_section_to_js(section));
    }
    set_value(&object, "sections", sections.into());
    Ok(object.into())
}

fn snapshot_section_to_js(section: &usb_auto::SnapshotSection) -> JsValue {
    let object = Object::new();
    match section {
        usb_auto::SnapshotSection::DeviceInfo { version } => {
            set_str(&object, "type", "deviceInfo");
            set_str(&object, "version", version.as_str());
        }
        usb_auto::SnapshotSection::Persistence { state } => {
            set_str(&object, "type", "persistence");
            set_str(&object, "state", persistence_state_to_str(*state));
        }
        usb_auto::SnapshotSection::LoraStatus(status) => {
            set_str(&object, "type", "loraStatus");
            set_value(&object, "status", interface_status_to_js(*status).into());
        }
        usb_auto::SnapshotSection::UsbStatus(status) => {
            set_str(&object, "type", "usbStatus");
            set_value(&object, "status", interface_status_to_js(*status).into());
        }
        usb_auto::SnapshotSection::BleStatus {
            status,
            failure_reason,
        } => {
            set_str(&object, "type", "bleStatus");
            set_value(&object, "status", interface_status_to_js(*status).into());
            set_str(&object, "failureReason", failure_reason.as_str());
        }
        usb_auto::SnapshotSection::BleRecovery(body) => {
            set_str(&object, "type", "bleRecovery");
            set_u32(&object, "ingressPressure", body.ingress_pressure);
            set_u32(&object, "setupFailures", body.setup_failures);
            set_u32(&object, "transportClosures", body.transport_closures);
            set_u32(&object, "egressPressureEvents", body.egress_pressure_events);
            set_u32(&object, "memberCount", u32::from(body.member_count));
        }
        usb_auto::SnapshotSection::LoraSpectrum(body) => {
            set_str(&object, "type", "loraSpectrum");
            set_u32(
                &object,
                "channelBusyPerMille",
                u32::from(body.channel_busy_per_mille),
            );
            if let Some(noise) = body.noise_floor_dbm {
                set_i32(&object, "noiseFloorDbm", noise as i32);
            }
            if let Some(cca) = body.cca_threshold_dbm {
                set_i32(&object, "ccaThresholdDbm", cca as i32);
            }
            set_u32(&object, "deferrals", body.deferrals);
            set_u32(&object, "falsePreambles", body.false_preambles);
            set_u32(&object, "contentionTimeouts", body.contention_timeouts);
            set_u32(&object, "dutyHolds", body.duty_holds);
            set_u32(&object, "dutyTimeouts", body.duty_timeouts);
            set_u32(&object, "radioRecoveries", body.radio_recoveries);
        }
        usb_auto::SnapshotSection::RadioProfile(profile) => {
            set_str(&object, "type", "radioProfile");
            set_value(&object, "profile", profile_to_js(profile).into());
        }
        usb_auto::SnapshotSection::InterfaceCounts(counts) => {
            set_str(&object, "type", "interfaceCounts");
            let array = Array::new();
            for count in counts {
                let entry = Object::new();
                set_str(&entry, "kind", config_interface_to_str(count.kind));
                set_u32(&entry, "destinations", count.destinations);
                set_u32(&entry, "links", count.links);
                set_u32(&entry, "transportedLinks", count.transported_links);
                array.push(&entry.into());
            }
            set_value(&object, "counts", array.into());
        }
    }
    object.into()
}

fn interface_status_to_js(status: usb_auto::InterfaceStatusBody) -> Object {
    let object = Object::new();
    set_bool(&object, "enabled", status.enabled);
    set_str(
        &object,
        "connection",
        connection_state_to_str(status.connection),
    );
    set_u64(&object, "rxBytes", status.rx_bytes);
    set_u64(&object, "txBytes", status.tx_bytes);
    if let Some(airtime) = status.airtime {
        let air = Object::new();
        set_u32(&air, "shortPerMille", u32::from(airtime.short_per_mille));
        set_u32(&air, "longPerMille", u32::from(airtime.long_per_mille));
        set_value(&object, "airtime", air.into());
    }
    if let Some(rates) = status.transfer_rates {
        let transfer = Object::new();
        set_u32(&transfer, "rxBps", rates.rx_bps);
        set_u32(&transfer, "txBps", rates.tx_bps);
        set_value(&object, "transferRates", transfer.into());
    }
    object
}

/// Project a [`RadioProfile`] onto JS via its canonical wire form, so the
/// `/configure` editor and the device share one encoding. The wire bytes are
/// the source of truth for the enum codes; `region` adds the human label.
fn profile_to_js(profile: &RadioProfile) -> Object {
    let mut wire = [0u8; PROFILE_WIRE_LEN];
    profile.encode(&mut wire);
    let object = Object::new();
    set_u32(
        &object,
        "frequencyHz",
        u32::from_le_bytes([wire[0], wire[1], wire[2], wire[3]]),
    );
    set_u32(&object, "spreadingFactor", u32::from(wire[4]));
    set_u32(&object, "bandwidth", u32::from(wire[5]));
    set_u32(&object, "codingRate", u32::from(wire[6]));
    set_i32(&object, "txPowerDbm", i8::from_le_bytes([wire[7]]) as i32);
    set_u32(
        &object,
        "preamble",
        u32::from(u16::from_le_bytes([wire[8], wire[9]])),
    );
    set_u32(&object, "regionCode", u32::from(wire[10]));
    set_str(&object, "region", profile.region.label());
    object
}

fn connection_state_to_str(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Initializing => "initializing",
        ConnectionState::Connected => "connected",
        ConnectionState::Degraded => "degraded",
        ConnectionState::Reconnecting => "reconnecting",
        ConnectionState::Failed => "failed",
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Disabled => "disabled",
        ConnectionState::Unknown => "unknown",
    }
}

fn persistence_state_to_str(state: usb_auto::SnapshotPersistence) -> &'static str {
    match state {
        usb_auto::SnapshotPersistence::Durable => "durable",
        usb_auto::SnapshotPersistence::Deferred => "deferred",
        usb_auto::SnapshotPersistence::Failed => "failed",
    }
}

fn config_interface_to_str(interface: usb_auto::ConfigInterface) -> &'static str {
    match interface {
        usb_auto::ConfigInterface::Lora => "lora",
        usb_auto::ConfigInterface::Usb => "usb",
        usb_auto::ConfigInterface::Ble => "ble",
    }
}

pub(crate) fn bluetooth_control_to_js(control: bluetooth_contract::Control) -> JsValue {
    let object = Object::new();
    match control {
        bluetooth_contract::Control::Hello {
            identity,
            endpoint,
            capabilities,
            peer_rssi,
        } => {
            set_str(&object, "type", "hello");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
        }
        bluetooth_contract::Control::Welcome {
            identity,
            endpoint,
            capabilities,
            peer_rssi,
        } => {
            set_str(&object, "type", "welcome");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
        }
        bluetooth_contract::Control::Close { reason } => {
            set_str(&object, "type", "close");
            set_str(&object, "reason", &format!("{reason:?}"));
        }
    }
    object.into()
}

fn outbound_target_to_js(target: OutboundTarget) -> JsValue {
    let object = Object::new();
    match target {
        OutboundTarget::Interface(interface) => {
            set_str(&object, "type", "interface");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
        OutboundTarget::Broadcast { supervisor, fan } => {
            set_str(&object, "type", "broadcast");
            set_str(
                &object,
                "supervisorKind",
                interface_kind_name(Some(supervisor)),
            );
            set_value(&object, "fan", fan_target_to_js(fan));
        }
    }
    object.into()
}

fn fan_target_to_js(fan: FanTarget) -> JsValue {
    let object = Object::new();
    match fan {
        FanTarget::All => set_str(&object, "type", "all"),
        FanTarget::Only(interface) => {
            set_str(&object, "type", "only");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
        FanTarget::AllExcept(interface) => {
            set_str(&object, "type", "allExcept");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
    }
    object.into()
}

pub(crate) fn interface_kind_name(kind: Option<InterfaceKind>) -> &'static str {
    match kind {
        Some(kind) => kind.name(),
        None => "unknown",
    }
}

pub(crate) fn set_str(object: &Object, key: &str, value: &str) {
    set_value(object, key, JsValue::from_str(value));
}

pub(crate) fn set_u32(object: &Object, key: &str, value: u32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_i32(object: &Object, key: &str, value: i32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_bool(object: &Object, key: &str, value: bool) {
    set_value(object, key, JsValue::from_bool(value));
}

pub(crate) fn set_u64(object: &Object, key: &str, value: u64) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_bigint(object: &Object, key: &str, value: u64) {
    set_value(object, key, BigInt::from(value).into());
}

pub(crate) fn set_usize(object: &Object, key: &str, value: usize) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_bytes(object: &Object, key: &str, value: &[u8]) {
    set_value(object, key, Uint8Array::from(value).into());
}

pub(crate) fn set_value(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
