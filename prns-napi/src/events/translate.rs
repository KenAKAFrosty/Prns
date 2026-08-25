use napi::bindgen_prelude::{BigInt, Buffer, Object};
use napi::Env;

use super::owned::OwnedEvent;

fn bytes(value: &[u8]) -> Buffer {
    Buffer::from(value.to_vec())
}

pub fn event_to_object(env: &Env, event: OwnedEvent) -> napi::Result<Object<'static>> {
    let mut object = Object::new(env)?;
    match event {
        OwnedEvent::Announce {
            app_data,
            destination,
            hops,
            source_interface,
        } => {
            object.set("type", "announce")?;
            object.set("appData", Buffer::from(app_data))?;
            object.set("destination", bytes(&destination))?;
            object.set("hops", u32::from(hops))?;
            object.set("sourceInterface", bytes(&source_interface))?;
        }
        OwnedEvent::SingleDelivery {
            destination,
            plaintext,
            source_interface,
        } => {
            object.set("type", "singleDelivery")?;
            object.set("destination", bytes(&destination))?;
            object.set("plaintext", Buffer::from(plaintext))?;
            object.set("sourceInterface", bytes(&source_interface))?;
        }
        OwnedEvent::LinkDelivery {
            link_id,
            plaintext,
            source_interface,
        } => {
            object.set("type", "linkDelivery")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("plaintext", Buffer::from(plaintext))?;
            object.set("sourceInterface", bytes(&source_interface))?;
        }
        OwnedEvent::Request {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            rtt_millis,
            data,
        } => {
            object.set("type", "request")?;
            object.set("destination", bytes(&destination))?;
            object.set("linkId", bytes(&link_id))?;
            object.set("requestId", bytes(&request_id))?;
            if let Some(requester) = requester {
                object.set("requester", bytes(&requester))?;
            }
            object.set("pathHash", bytes(&path_hash))?;
            object.set("rttMillis", rtt_millis as f64)?;
            object.set("data", Buffer::from(data))?;
            let mut token = Object::new(env)?;
            token.set("linkId", bytes(&link_id))?;
            token.set("requestId", bytes(&request_id))?;
            token.set("rttMillis", rtt_millis as f64)?;
            object.set("token", token)?;
        }
        OwnedEvent::Response {
            link_id,
            request_id,
            data,
        } => {
            object.set("type", "response")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("requestId", bytes(&request_id))?;
            object.set("data", Buffer::from(data))?;
        }
        OwnedEvent::ResponseSegment {
            link_id,
            request_id,
            segment_index,
            total_segments,
            data,
        } => {
            object.set("type", "responseSegment")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("requestId", bytes(&request_id))?;
            object.set("segmentIndex", segment_index as f64)?;
            object.set("totalSegments", total_segments as f64)?;
            object.set("data", Buffer::from(data))?;
        }
        OwnedEvent::Resource {
            link_id,
            hash,
            metadata,
            data,
        } => {
            object.set("type", "resourceReceived")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("hash", Buffer::from(hash))?;
            if let Some(metadata) = metadata {
                object.set("metadata", Buffer::from(metadata))?;
            }
            object.set("data", Buffer::from(data))?;
        }
        OwnedEvent::ResourceSegment {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => {
            object.set("type", "resourceSegment")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("originalHash", Buffer::from(original_hash))?;
            object.set("segmentIndex", segment_index as f64)?;
            object.set("totalSegments", total_segments as f64)?;
            if let Some(metadata) = metadata {
                object.set("metadata", Buffer::from(metadata))?;
            }
            object.set("data", Buffer::from(data))?;
        }
        OwnedEvent::ChannelMessage {
            link_id,
            message_type,
            data,
        } => {
            object.set("type", "channelMessage")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("messageType", message_type)?;
            object.set("data", Buffer::from(data))?;
        }
        OwnedEvent::LinkEstablished {
            link_id,
            rtt_millis,
        } => {
            object.set("type", "linkEstablished")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("rttMillis", rtt_millis as f64)?;
        }
        OwnedEvent::PeerIdentified { link_id, identity } => {
            object.set("type", "peerIdentified")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("identity", bytes(&identity))?;
        }
        OwnedEvent::LinkClosed { link_id, reason } => {
            object.set("type", "linkClosed")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("reason", reason)?;
        }
        OwnedEvent::CommandSettled { id, settlement } => {
            object.set("type", "commandSettled")?;
            object.set("id", id as f64)?;
            object.set("settlement", settlement)?;
        }
        OwnedEvent::SelfRatchetRotated { destination } => {
            object.set("type", "selfRatchetRotated")?;
            object.set("destination", bytes(&destination))?;
        }
        OwnedEvent::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => {
            object.set("type", "announceHeldDropped")?;
            object.set("destination", bytes(&destination))?;
            object.set("sourceInterface", bytes(&source_interface))?;
            object.set("cause", cause)?;
        }
        OwnedEvent::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            object.set("type", "linkInterfaceMismatch")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("attachedInterface", bytes(&attached_interface))?;
            object.set("arrivedOn", bytes(&arrived_on))?;
        }
        OwnedEvent::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => {
            object.set("type", "resourceAssembled")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("originalHash", Buffer::from(original_hash))?;
            object.set("totalSizeBytes", BigInt::from(total_size_bytes))?;
        }
        OwnedEvent::ResourceFailed {
            link_id,
            hash,
            cause,
        } => {
            object.set("type", "resourceFailed")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("hash", Buffer::from(hash))?;
            object.set("cause", cause)?;
        }
        OwnedEvent::RouteRemoved { destination, kind } => {
            object.set("type", kind)?;
            object.set("destination", bytes(&destination))?;
        }
        OwnedEvent::ResourceSendProgress {
            link_id,
            transferred_bytes,
            total_bytes,
            physical_transferred_bytes,
            segment_index,
            total_segments,
        } => {
            object.set("type", "resourceSendProgress")?;
            object.set("linkId", bytes(&link_id))?;
            object.set("transferredBytes", BigInt::from(transferred_bytes))?;
            object.set("totalBytes", BigInt::from(total_bytes))?;
            object.set(
                "physicalTransferredBytes",
                BigInt::from(physical_transferred_bytes),
            )?;
            object.set("segmentIndex", segment_index as f64)?;
            object.set("totalSegments", total_segments as f64)?;
        }
        OwnedEvent::PersistenceRestored {
            routes,
            destination_identities,
            tunnels,
            ratchets,
            refused,
            dropped,
        } => {
            object.set("type", "persistenceRestored")?;
            object.set("routes", routes as f64)?;
            object.set("destinationIdentities", destination_identities as f64)?;
            object.set("tunnels", tunnels as f64)?;
            object.set("ratchets", ratchets as f64)?;
            object.set("refused", refused as f64)?;
            object.set("dropped", dropped as f64)?;
        }
        OwnedEvent::PersistenceFlushed { cause, target } => {
            object.set("type", "persistenceFlushed")?;
            object.set("cause", cause)?;
            object.set("target", target)?;
        }
        OwnedEvent::PersistenceFlushFailed { cause, target } => {
            object.set("type", "persistenceFlushFailed")?;
            object.set("cause", cause)?;
            object.set("target", target)?;
        }
        OwnedEvent::Uncategorized { kind, detail } => {
            object.set("type", kind)?;
            object.set("detail", detail)?;
        }
        OwnedEvent::EventOverflow {
            dropped_diagnostics,
        } => {
            object.set("type", "eventOverflow")?;
            object.set("droppedDiagnostics", dropped_diagnostics as f64)?;
        }
        OwnedEvent::EventBackpressureExceeded {
            rejected_event_bytes,
        } => {
            object.set("type", "eventBackpressureExceeded")?;
            object.set("rejectedEventBytes", rejected_event_bytes as f64)?;
        }
        OwnedEvent::NodeStopped { cause } => {
            object.set("type", "nodeStopped")?;
            object.set("cause", cause)?;
        }
    }
    Ok(object)
}
