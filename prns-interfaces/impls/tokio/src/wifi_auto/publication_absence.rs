use std::time::{Duration, Instant};

use prns_core::interfaces::wifi_auto::DiscoveryTransport;

/// Keep a registered UDP service alive across brief link-local IPv6 discovery gaps.
const UDP_PUBLICATION_EMPTY_GRACE: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PublicationPresence {
    Unregistered,
    Registered,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EmptyPublicationDisposition {
    AlreadyAbsent,
    RetainDuringGrace,
    Withdraw,
}

pub(super) struct PublicationAbsence {
    udp_empty_since: Option<Instant>,
}

impl PublicationAbsence {
    pub(super) const fn new() -> Self {
        Self {
            udp_empty_since: None,
        }
    }

    pub(super) fn observe_available(&mut self, discovery_transport: DiscoveryTransport) {
        match discovery_transport {
            DiscoveryTransport::Tcp => {}
            DiscoveryTransport::Udp => self.udp_empty_since = None,
        }
    }

    pub(super) fn observe_empty(
        &mut self,
        discovery_transport: DiscoveryTransport,
        publication_presence: PublicationPresence,
        observed_at: Instant,
    ) -> EmptyPublicationDisposition {
        match (discovery_transport, publication_presence) {
            (DiscoveryTransport::Tcp, PublicationPresence::Unregistered) => {
                EmptyPublicationDisposition::AlreadyAbsent
            }
            (DiscoveryTransport::Tcp, PublicationPresence::Registered) => {
                EmptyPublicationDisposition::Withdraw
            }
            (DiscoveryTransport::Udp, PublicationPresence::Unregistered) => {
                self.udp_empty_since = None;
                EmptyPublicationDisposition::AlreadyAbsent
            }
            (DiscoveryTransport::Udp, PublicationPresence::Registered) => {
                let Some(empty_since) = self.udp_empty_since else {
                    self.udp_empty_since = Some(observed_at);
                    return EmptyPublicationDisposition::RetainDuringGrace;
                };
                if observed_at.saturating_duration_since(empty_since) < UDP_PUBLICATION_EMPTY_GRACE
                {
                    return EmptyPublicationDisposition::RetainDuringGrace;
                }
                self.udp_empty_since = None;
                EmptyPublicationDisposition::Withdraw
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_publication_grace_is_udp_specific_and_expires_at_its_boundary() {
        let observed_at = Instant::now();
        let mut publication_absence = PublicationAbsence::new();

        let tcp_disposition = publication_absence.observe_empty(
            DiscoveryTransport::Tcp,
            PublicationPresence::Registered,
            observed_at,
        );
        let udp_initial_disposition = publication_absence.observe_empty(
            DiscoveryTransport::Udp,
            PublicationPresence::Registered,
            observed_at,
        );
        publication_absence.observe_available(DiscoveryTransport::Tcp);
        let udp_before_boundary = publication_absence.observe_empty(
            DiscoveryTransport::Udp,
            PublicationPresence::Registered,
            observed_at + UDP_PUBLICATION_EMPTY_GRACE - Duration::from_millis(1),
        );
        publication_absence.observe_available(DiscoveryTransport::Tcp);
        let udp_at_boundary = publication_absence.observe_empty(
            DiscoveryTransport::Udp,
            PublicationPresence::Registered,
            observed_at + UDP_PUBLICATION_EMPTY_GRACE,
        );
        let dispositions = [
            tcp_disposition,
            udp_initial_disposition,
            udp_before_boundary,
            udp_at_boundary,
        ];

        assert_eq!(
            dispositions,
            [
                EmptyPublicationDisposition::Withdraw,
                EmptyPublicationDisposition::RetainDuringGrace,
                EmptyPublicationDisposition::RetainDuringGrace,
                EmptyPublicationDisposition::Withdraw,
            ]
        );
    }

    #[test]
    fn udp_publication_grace_arms_only_while_registered_and_resets_on_recovery() {
        let observed_at = Instant::now();
        let mut publication_absence = PublicationAbsence::new();

        assert_eq!(
            publication_absence.observe_empty(
                DiscoveryTransport::Udp,
                PublicationPresence::Unregistered,
                observed_at,
            ),
            EmptyPublicationDisposition::AlreadyAbsent
        );
        assert_eq!(
            publication_absence.observe_empty(
                DiscoveryTransport::Udp,
                PublicationPresence::Registered,
                observed_at + UDP_PUBLICATION_EMPTY_GRACE,
            ),
            EmptyPublicationDisposition::RetainDuringGrace,
            "an absent registration cannot start a stale-publication deadline"
        );

        publication_absence.observe_available(DiscoveryTransport::Udp);
        let second_gap = observed_at + UDP_PUBLICATION_EMPTY_GRACE * 2;
        assert_eq!(
            publication_absence.observe_empty(
                DiscoveryTransport::Udp,
                PublicationPresence::Registered,
                second_gap,
            ),
            EmptyPublicationDisposition::RetainDuringGrace
        );
        assert_eq!(
            publication_absence.observe_empty(
                DiscoveryTransport::Udp,
                PublicationPresence::Registered,
                second_gap + UDP_PUBLICATION_EMPTY_GRACE,
            ),
            EmptyPublicationDisposition::Withdraw,
            "a recovered publication receives a fresh grace interval"
        );
    }
}
