use prns_host::EventDelivery;

pub enum OwnedEvent {
    Announce {
        app_data: Vec<u8>,
        destination: [u8; 16],
        hops: u8,
        source_interface: [u8; 8],
    },
    SingleDelivery {
        destination: [u8; 16],
        plaintext: Vec<u8>,
        source_interface: [u8; 8],
    },
    LinkDelivery {
        link_id: [u8; 16],
        plaintext: Vec<u8>,
        source_interface: [u8; 8],
    },
    Request {
        destination: [u8; 16],
        link_id: [u8; 16],
        request_id: [u8; 16],
        requester: Option<[u8; 16]>,
        path_hash: [u8; 16],
        rtt_millis: u64,
        data: Vec<u8>,
    },
    Response {
        link_id: [u8; 16],
        request_id: [u8; 16],
        data: Vec<u8>,
    },
    ResponseSegment {
        link_id: [u8; 16],
        request_id: [u8; 16],
        segment_index: u64,
        total_segments: u64,
        data: Vec<u8>,
    },
    Resource {
        link_id: [u8; 16],
        hash: Vec<u8>,
        metadata: Option<Vec<u8>>,
        data: Vec<u8>,
    },
    ResourceSegment {
        link_id: [u8; 16],
        original_hash: Vec<u8>,
        segment_index: u64,
        total_segments: u64,
        metadata: Option<Vec<u8>>,
        data: Vec<u8>,
    },
    ChannelMessage {
        link_id: [u8; 16],
        message_type: u16,
        data: Vec<u8>,
    },
    LinkEstablished {
        link_id: [u8; 16],
        rtt_millis: u64,
    },
    PeerIdentified {
        link_id: [u8; 16],
        identity: [u8; 16],
    },
    LinkClosed {
        link_id: [u8; 16],
        reason: &'static str,
    },
    CommandSettled {
        id: u64,
        settlement: String,
    },
    SelfRatchetRotated {
        destination: [u8; 16],
    },
    AnnounceHeldDropped {
        destination: [u8; 16],
        source_interface: [u8; 8],
        cause: String,
    },
    LinkInterfaceMismatch {
        link_id: [u8; 16],
        attached_interface: [u8; 8],
        arrived_on: [u8; 8],
    },
    ResourceAssembled {
        link_id: [u8; 16],
        original_hash: Vec<u8>,
        total_size_bytes: u64,
    },
    ResourceFailed {
        link_id: [u8; 16],
        hash: Vec<u8>,
        cause: String,
    },
    RouteRemoved {
        destination: [u8; 16],
        kind: &'static str,
    },
    ResourceSendProgress {
        link_id: [u8; 16],
        transferred_bytes: u64,
        total_bytes: u64,
        physical_transferred_bytes: u64,
        segment_index: u64,
        total_segments: u64,
    },
    PersistenceRestored {
        routes: u64,
        destination_identities: u64,
        tunnels: u64,
        ratchets: u64,
        refused: u64,
        dropped: u64,
    },
    PersistenceFlushed {
        cause: &'static str,
        target: &'static str,
    },
    PersistenceFlushFailed {
        cause: &'static str,
        target: &'static str,
    },
    Uncategorized {
        kind: String,
        detail: String,
    },
    EventOverflow {
        dropped_diagnostics: u64,
    },
    EventBackpressureExceeded {
        rejected_event_bytes: u64,
    },
    NodeStopped {
        cause: String,
    },
}

impl OwnedEvent {
    pub fn application_bytes(&self) -> Option<usize> {
        match self {
            Self::SingleDelivery { plaintext, .. } | Self::LinkDelivery { plaintext, .. } => {
                Some(plaintext.len())
            }
            Self::Request { data, .. }
            | Self::Response { data, .. }
            | Self::ResponseSegment { data, .. } => Some(data.len()),
            Self::Resource {
                hash,
                metadata,
                data,
                ..
            } => Some(
                hash.len()
                    .saturating_add(metadata.as_ref().map_or(0, Vec::len))
                    .saturating_add(data.len()),
            ),
            Self::ResourceSegment {
                original_hash,
                metadata,
                data,
                ..
            } => Some(
                original_hash
                    .len()
                    .saturating_add(metadata.as_ref().map_or(0, Vec::len))
                    .saturating_add(data.len()),
            ),
            Self::ChannelMessage { data, .. } => Some(data.len()),
            _ => None,
        }
    }

    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::EventBackpressureExceeded { .. } | Self::NodeStopped { .. }
        )
    }
}

impl EventDelivery for OwnedEvent {
    fn application_bytes(&self) -> Option<usize> {
        Self::application_bytes(self)
    }

    fn terminal(&self) -> bool {
        Self::terminal(self)
    }

    fn diagnostic_gap(dropped_diagnostics: u64) -> Self {
        Self::EventOverflow {
            dropped_diagnostics,
        }
    }
}

impl OwnedEvent {
    pub fn capture_host_application(event: prns_host::ApplicationEvent) -> Option<Self> {
        match event {
            prns_host::ApplicationEvent::SingleDelivery(event) => Some(Self::SingleDelivery {
                destination: event.destination.into_bytes(),
                plaintext: event.plaintext,
                source_interface: event.source_interface.into_bytes(),
            }),
            prns_host::ApplicationEvent::LinkDelivery(event) => Some(Self::LinkDelivery {
                link_id: event.link_id.into_bytes(),
                plaintext: event.plaintext,
                source_interface: event.source_interface.into_bytes(),
            }),
            prns_host::ApplicationEvent::Request(event) => Some(Self::Request {
                destination: event.destination.into_bytes(),
                link_id: event.link_id.into_bytes(),
                request_id: event.request_id.into_bytes(),
                requester: event.requester.map(prns_host::IdentityHash::into_bytes),
                path_hash: event.path_hash.into_bytes(),
                rtt_millis: event.rtt_millis,
                data: event.data,
            }),
            prns_host::ApplicationEvent::Response(event) => Some(Self::Response {
                link_id: event.link_id.into_bytes(),
                request_id: event.request_id.into_bytes(),
                data: event.data,
            }),
            prns_host::ApplicationEvent::ResponseSegment(event) => Some(Self::ResponseSegment {
                link_id: event.link_id.into_bytes(),
                request_id: event.request_id.into_bytes(),
                segment_index: event.segment_index,
                total_segments: event.total_segments,
                data: event.data,
            }),
            prns_host::ApplicationEvent::ResourceSegment(event) => Some(Self::ResourceSegment {
                link_id: event.link_id.into_bytes(),
                original_hash: event.original_hash.into_bytes().to_vec(),
                segment_index: event.segment_index,
                total_segments: event.total_segments,
                metadata: event.metadata,
                data: event.data,
            }),
            prns_host::ApplicationEvent::ChannelMessage(event) => Some(Self::ChannelMessage {
                link_id: event.link_id.into_bytes(),
                message_type: event.message_type,
                data: event.data,
            }),
            prns_host::ApplicationEvent::ResourceAvailable(_)
            | prns_host::ApplicationEvent::ResourceNeedsDecompression(_) => None,
        }
    }

    pub fn capture_host_resource(event: prns_host::ResourceAvailable, body: Vec<u8>) -> Self {
        Self::Resource {
            link_id: event.link_id.into_bytes(),
            hash: event.hash.into_bytes().to_vec(),
            metadata: event.metadata,
            data: body,
        }
    }

    pub fn capture_host_diagnostic(event: prns_host::DiagnosticEvent) -> Self {
        match event {
            prns_host::DiagnosticEvent::AnnounceHeard {
                app_data,
                destination,
                hops,
                source_interface,
            } => Self::Announce {
                app_data,
                destination: destination.into_bytes(),
                hops,
                source_interface: source_interface.into_bytes(),
            },
            prns_host::DiagnosticEvent::LinkEstablished {
                link_id,
                rtt_millis,
            } => Self::LinkEstablished {
                link_id: link_id.into_bytes(),
                rtt_millis,
            },
            prns_host::DiagnosticEvent::PeerIdentified { link_id, identity } => {
                Self::PeerIdentified {
                    link_id: link_id.into_bytes(),
                    identity: identity.into_bytes(),
                }
            }
            prns_host::DiagnosticEvent::LinkClosed { link_id, reason } => Self::LinkClosed {
                link_id: link_id.into_bytes(),
                reason: match reason {
                    prns_host::LinkClosedReason::Timeout => "timeout",
                    prns_host::LinkClosedReason::PeerClosed => "peerClosed",
                    prns_host::LinkClosedReason::MalformedRtt => "malformedRtt",
                },
            },
            prns_host::DiagnosticEvent::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            } => Self::LinkInterfaceMismatch {
                link_id: link_id.into_bytes(),
                attached_interface: attached_interface.into_bytes(),
                arrived_on: arrived_on.into_bytes(),
            },
            prns_host::DiagnosticEvent::ResourceAssembled {
                link_id,
                original_hash,
                total_size_bytes,
            } => Self::ResourceAssembled {
                link_id: link_id.into_bytes(),
                original_hash: original_hash.into_bytes().to_vec(),
                total_size_bytes,
            },
            prns_host::DiagnosticEvent::ResourceFailed {
                link_id,
                hash,
                cause,
            } => Self::ResourceFailed {
                link_id: link_id.into_bytes(),
                hash: hash.into_bytes().to_vec(),
                cause,
            },
            prns_host::DiagnosticEvent::ResourceSendProgress {
                link_id,
                transferred_bytes,
                total_bytes,
                physical_transferred_bytes,
                segment_index,
                total_segments,
            } => Self::ResourceSendProgress {
                link_id: link_id.into_bytes(),
                transferred_bytes,
                total_bytes,
                physical_transferred_bytes,
                segment_index,
                total_segments,
            },
            prns_host::DiagnosticEvent::SelfRatchetRotated { destination } => {
                Self::SelfRatchetRotated {
                    destination: destination.into_bytes(),
                }
            }
            prns_host::DiagnosticEvent::AnnounceHeldDropped {
                destination,
                source_interface,
                cause,
            } => Self::AnnounceHeldDropped {
                destination: destination.into_bytes(),
                source_interface: source_interface.into_bytes(),
                cause,
            },
            prns_host::DiagnosticEvent::Delivered { detail } => Self::Uncategorized {
                kind: "delivered".to_string(),
                detail,
            },
            prns_host::DiagnosticEvent::RouteExpired { destination } => Self::RouteRemoved {
                destination: destination.into_bytes(),
                kind: "routeExpired",
            },
            prns_host::DiagnosticEvent::RouteEvicted { destination } => Self::RouteRemoved {
                destination: destination.into_bytes(),
                kind: "routeEvicted",
            },
            prns_host::DiagnosticEvent::RouteInterfaceGone { destination } => Self::RouteRemoved {
                destination: destination.into_bytes(),
                kind: "routeInterfaceGone",
            },
            prns_host::DiagnosticEvent::RouteDropped { destination } => Self::RouteRemoved {
                destination: destination.into_bytes(),
                kind: "routeDropped",
            },
            prns_host::DiagnosticEvent::BackendDiagnostic { kind, detail } => {
                if kind == "commandSettled" {
                    if let Some((id, settlement)) = detail.split_once(':') {
                        if let Ok(id) = id.parse() {
                            return Self::CommandSettled {
                                id,
                                settlement: settlement.to_string(),
                            };
                        }
                    }
                }
                Self::Uncategorized { kind, detail }
            }
            prns_host::DiagnosticEvent::PersistenceRestored {
                routes,
                destination_identities,
                tunnels,
                ratchets,
                refused,
                dropped,
            } => Self::PersistenceRestored {
                routes,
                destination_identities,
                tunnels,
                ratchets,
                refused,
                dropped,
            },
            prns_host::DiagnosticEvent::PersistenceFlushed { cause, target } => {
                Self::PersistenceFlushed {
                    cause: host_persistence_cause(cause),
                    target: host_persistence_target(target),
                }
            }
            prns_host::DiagnosticEvent::PersistenceFlushFailed { cause, target } => {
                Self::PersistenceFlushFailed {
                    cause: host_persistence_cause(cause),
                    target: host_persistence_target(target),
                }
            }
        }
    }
}

fn host_persistence_cause(cause: prns_host::PersistenceFlushCause) -> &'static str {
    match cause {
        prns_host::PersistenceFlushCause::Startup => "startup",
        prns_host::PersistenceFlushCause::Interval => "interval",
        prns_host::PersistenceFlushCause::RouteChange => "routeChange",
        prns_host::PersistenceFlushCause::RatchetRotation => "ratchetRotation",
        prns_host::PersistenceFlushCause::Shutdown => "shutdown",
    }
}

fn host_persistence_target(target: prns_host::PersistenceFlushTarget) -> &'static str {
    match target {
        prns_host::PersistenceFlushTarget::RoutingState => "routingState",
        prns_host::PersistenceFlushTarget::Ratchets => "ratchets",
    }
}
