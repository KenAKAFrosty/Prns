//! Opt-in, bounded metadata observations for Link establishment.
//!
//! The callback is synchronous and must be nonblocking. It must not retain keys,
//! packet bodies or destination identities: none are provided here. Link IDs are
//! public, transient wire identifiers and intentionally allow one handshake to be
//! correlated across hops. Exporters should salt interface tags per capture.
//! Events establish core decisions, never carrier admission or RF transmission.
use crate::engine::{CommandId, EngineState, InstantMillis};
use crate::interfaces::{BitrateBps, InterfaceId};
use crate::routing::ingress::IgnoreReason;
use crate::routing::links::establish::{
    WriteEstablishLinkRejection, WriteLinkProofError, WriteLinkRttError,
};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

/// Maximum data callbacks per engine lifetime, including sink rejections.
/// One additional callback is reserved for the terminal limit notice.
pub const LINK_HANDSHAKE_OBSERVATION_LIMIT: u64 = 4096;
pub type LinkHandshakeObserver = fn(LinkHandshakeEvent) -> bool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHandshakeEvent {
    pub sequence: u64,
    pub dropped_before: u64,
    /// `None` when the core entry point has no completion clock. In particular,
    /// RequestPending.requested_at is not the crypto-completion/egress time.
    pub at: Option<InstantMillis>,
    pub link_id: Option<LinkId>,
    pub command: Option<CommandId>,
    pub interface: Option<InterfaceId>,
    pub phase: LinkHandshakePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHandshakePhase {
    RequestPending {
        requested_at: InstantMillis,
        timeout_at: InstantMillis,
        hops: u8,
        bitrate: Option<BitrateBps>,
        first_hop_floor_ms: Option<u64>,
        computed_timeout_ms: u64,
        /// Process-local route incarnation, not a destination identifier.
        route_generation: u32,
    },
    RequestRejected(WriteEstablishLinkRejection),
    RequestIngress(LinkHandshakeIngress),
    ProofIngress(LinkHandshakeIngress),
    /// Responding state installed and proof serialized; no egress claim.
    ProofPrepared,
    ProofRejected(WriteLinkProofError),
    ProofCompletion(LinkProofCompletion),
    InitiatorTimeout {
        requested_at: InstantMillis,
        route_generation: u32,
        route: LinkTimeoutRouteDisposition,
    },
    RelayPending {
        timeout_at: InstantMillis,
        remaining_hops: u8,
        taken_hops: u8,
        bitrate: Option<BitrateBps>,
        route_generation: u32,
    },
    RelayTimeout {
        timeout_at: InstantMillis,
        route_generation: u32,
    },
    ResponderTimeout,
    ObservationLimitReached {
        limit: u64,
        dropped_at_least: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHandshakeIngress {
    Accepted,
    VerificationPending,
    Forwarded,
    Ignored(IgnoreReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProofCompletion {
    /// Initiator activated and LRRTT serialized, before its send directive.
    Activated,
    EgressDisabled,
    InvalidSignature,
    WriteRejected(WriteLinkRttError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkTimeoutRouteDisposition {
    Removed,
    /// No matching route, a replacement, or more recent route activity.
    Preserved,
    TransportMarkedUnresponsive,
    TransportPreserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHandshakeSnapshot {
    pub enabled: bool,
    pub observed: u64,
    /// Admitted data events; excludes the reserved terminal notice.
    pub emitted: u64,
    pub dropped: u64,
    pub limit: u64,
    /// Whether the separately reserved terminal notice was admitted by the sink.
    pub limit_notice_delivered: bool,
}

#[derive(Default)]
pub(crate) struct LinkHandshakeDiagnostics {
    observer: Option<LinkHandshakeObserver>,
    observed: u64,
    emitted: u64,
    dropped: u64,
    limit_notice_delivered: bool,
}

const _: () = {
    assert!(core::mem::size_of::<LinkHandshakeDiagnostics>() <= 40);
    assert!(core::mem::size_of::<LinkHandshakeEvent>() <= 144);
};

impl LinkHandshakeDiagnostics {
    pub(crate) fn observe(
        &mut self,
        at: Option<InstantMillis>,
        link_id: Option<LinkId>,
        command: Option<CommandId>,
        interface: Option<InterfaceId>,
        phase: LinkHandshakePhase,
    ) {
        let Some(observer) = self.observer else {
            return;
        };
        self.observed = self.observed.saturating_add(1);
        if self.observed > LINK_HANDSHAKE_OBSERVATION_LIMIT {
            self.dropped = self.dropped.saturating_add(1);
            if self.observed == LINK_HANDSHAKE_OBSERVATION_LIMIT + 1 {
                self.limit_notice_delivered = observer(LinkHandshakeEvent {
                    sequence: self.observed,
                    dropped_before: self.dropped,
                    at,
                    link_id: None,
                    command: None,
                    interface: None,
                    phase: LinkHandshakePhase::ObservationLimitReached {
                        limit: LINK_HANDSHAKE_OBSERVATION_LIMIT,
                        dropped_at_least: self.dropped,
                    },
                });
            }
            return;
        }
        if observer(LinkHandshakeEvent {
            sequence: self.observed,
            dropped_before: self.dropped,
            at,
            link_id,
            command,
            interface,
            phase,
        }) {
            self.emitted = self.emitted.saturating_add(1);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// Installs or disables a process-local observer. Disabled by default.
    /// Replacing/disabling it never resets sequence numbers or lifetime budgets.
    pub fn set_link_handshake_observer(&mut self, observer: Option<LinkHandshakeObserver>) {
        self.link_handshake_diagnostics.observer = observer;
    }

    /// Records a runtime worker's failed signature verification. This diagnostic
    /// hook changes no Link, route, deadline or command settlement state. Call only
    /// after verifying the exact owed proof, before discarding that completion.
    pub fn observe_link_proof_rejected(
        &mut self,
        link_id: LinkId,
        source_interface: InterfaceId,
        now: InstantMillis,
    ) {
        let command = match self.links.phase_for(&link_id) {
            Some(crate::routing::links::table::LinkPhase::Pending { command_id, .. }) => {
                Some(*command_id)
            }
            _ => None,
        };
        self.link_handshake_diagnostics.observe(
            Some(now),
            Some(link_id),
            command,
            Some(source_interface),
            LinkHandshakePhase::ProofCompletion(LinkProofCompletion::InvalidSignature),
        );
    }

    /// Read even after the event budget is exhausted to distinguish silence from
    /// complete coverage. Sink admission is not durable log-storage confirmation.
    pub const fn link_handshake_snapshot(&self) -> LinkHandshakeSnapshot {
        let diagnostics = &self.link_handshake_diagnostics;
        LinkHandshakeSnapshot {
            enabled: diagnostics.observer.is_some(),
            observed: diagnostics.observed,
            emitted: diagnostics.emitted,
            dropped: diagnostics.dropped,
            limit: LINK_HANDSHAKE_OBSERVATION_LIMIT,
            limit_notice_delivered: diagnostics.limit_notice_delivered,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use std::cell::RefCell;
    std::thread_local! { static EVENTS: RefCell<std::vec::Vec<LinkHandshakeEvent>> = const { RefCell::new(std::vec::Vec::new()) }; }
    fn sink(event: LinkHandshakeEvent) -> bool {
        let accepted = event.sequence != 2;
        EVENTS.with(|events| events.borrow_mut().push(event));
        accepted
    }
    fn observe(state: &mut EngineState<TestStorageLayout>) {
        state.link_handshake_diagnostics.observe(
            None,
            None,
            None,
            None,
            LinkHandshakePhase::ResponderTimeout,
        );
    }
    #[test]
    fn diagnostic_layout_is_bounded() {
        fn copy<T: Copy>() {}
        copy::<LinkHandshakeEvent>();
        std::println!(
            "LinkHandshakeDiagnostics={} LinkHandshakeEvent={}",
            core::mem::size_of::<LinkHandshakeDiagnostics>(),
            core::mem::size_of::<LinkHandshakeEvent>()
        );
        assert!(core::mem::size_of::<LinkHandshakeDiagnostics>() <= 40);
        assert!(core::mem::size_of::<LinkHandshakeEvent>() <= 144);
    }

    #[test]
    fn disabled_bounded_and_rejected_observations_keep_explicit_coverage() {
        EVENTS.with(|events| events.borrow_mut().clear());
        let mut state = EngineState::default();
        observe(&mut state);
        assert_eq!(state.link_handshake_snapshot().observed, 0);
        state.set_link_handshake_observer(Some(sink));
        for _ in 0..LINK_HANDSHAKE_OBSERVATION_LIMIT + 3 {
            observe(&mut state);
        }
        let snapshot = state.link_handshake_snapshot();
        assert_eq!(snapshot.observed, LINK_HANDSHAKE_OBSERVATION_LIMIT + 3);
        assert_eq!(snapshot.emitted, LINK_HANDSHAKE_OBSERVATION_LIMIT - 1);
        assert_eq!(snapshot.dropped, 4);
        EVENTS.with(|events| {
            let events = events.borrow();
            assert_eq!(events.len() as u64, LINK_HANDSHAKE_OBSERVATION_LIMIT + 1);
            assert!(matches!(
                events.last().unwrap().phase,
                LinkHandshakePhase::ObservationLimitReached { .. }
            ));
            assert_eq!(events[2].dropped_before, 1);
            assert_eq!(events[2].sequence, 3);
        });
        state.set_link_handshake_observer(None);
        observe(&mut state);
        assert_eq!(state.link_handshake_snapshot().observed, snapshot.observed);
        state.set_link_handshake_observer(Some(sink));
        observe(&mut state);
        assert_eq!(state.link_handshake_snapshot().dropped, 5);
    }
}
