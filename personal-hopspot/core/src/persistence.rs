#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PersistenceState {
    Durable,
    Recovered,
    Deferred,
    Failed,
}

impl PersistenceState {
    #[must_use]
    pub const fn encode(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn decode(value: u8) -> Self {
        match value {
            value if value == Self::Recovered as u8 => Self::Recovered,
            value if value == Self::Deferred as u8 => Self::Deferred,
            value if value == Self::Failed as u8 => Self::Failed,
            _ => Self::Durable,
        }
    }

    #[cfg(feature = "embedded")]
    #[must_use]
    pub const fn from_embedded_diagnostic(
        diagnostic: &personal_rns::runtime::EmbeddedPersistenceDiagnostic,
    ) -> Option<Self> {
        use personal_rns::runtime::EmbeddedPersistenceDiagnostic;

        match diagnostic {
            EmbeddedPersistenceDiagnostic::Restored(report) => {
                if report.warning.is_some()
                    || report.route_refused_count > 0
                    || report.route_dropped_count > 0
                    || report.ratchet_refused_count > 0
                {
                    Some(Self::Recovered)
                } else {
                    Some(Self::Durable)
                }
            }
            EmbeddedPersistenceDiagnostic::BatchPersisted {
                state_not_saved, ..
            }
            | EmbeddedPersistenceDiagnostic::CompactionCompleted {
                state_not_saved, ..
            } => {
                if *state_not_saved {
                    Some(Self::Deferred)
                } else {
                    Some(Self::Durable)
                }
            }
            EmbeddedPersistenceDiagnostic::CompactionStarted { .. } => None,
            EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. } => Some(Self::Deferred),
            EmbeddedPersistenceDiagnostic::WriteFailed { .. } => Some(Self::Failed),
        }
    }
}

#[cfg(all(test, feature = "embedded"))]
mod tests {
    use super::PersistenceState;
    use personal_rns::engine::InstantMillis;
    use personal_rns::persistence::FlashJournalWarning;
    use personal_rns::runtime::{
        EmbeddedPersistenceDiagnostic, EmbeddedPersistenceFailure,
        EmbeddedPersistenceRestoreReport, EmbeddedPersistenceTarget,
    };

    const EMPTY_RESTORE: EmbeddedPersistenceRestoreReport = EmbeddedPersistenceRestoreReport {
        logical_start: InstantMillis(0),
        route_seeded_count: 0,
        route_refused_count: 0,
        route_dropped_count: 0,
        ratchet_seeded_count: 0,
        ratchet_refused_count: 0,
        warning: None,
    };

    #[test]
    fn embedded_diagnostics_have_exhaustive_user_visible_health() {
        let cases = [
            (
                EmbeddedPersistenceDiagnostic::Restored(EMPTY_RESTORE),
                Some(PersistenceState::Durable),
            ),
            (
                EmbeddedPersistenceDiagnostic::Restored(EmbeddedPersistenceRestoreReport {
                    warning: Some(FlashJournalWarning::Corrupt),
                    ..EMPTY_RESTORE
                }),
                Some(PersistenceState::Recovered),
            ),
            (
                EmbeddedPersistenceDiagnostic::Restored(EmbeddedPersistenceRestoreReport {
                    route_refused_count: 1,
                    ..EMPTY_RESTORE
                }),
                Some(PersistenceState::Recovered),
            ),
            (
                EmbeddedPersistenceDiagnostic::Restored(EmbeddedPersistenceRestoreReport {
                    route_dropped_count: 1,
                    ..EMPTY_RESTORE
                }),
                Some(PersistenceState::Recovered),
            ),
            (
                EmbeddedPersistenceDiagnostic::Restored(EmbeddedPersistenceRestoreReport {
                    ratchet_refused_count: 1,
                    ..EMPTY_RESTORE
                }),
                Some(PersistenceState::Recovered),
            ),
            (
                EmbeddedPersistenceDiagnostic::BatchPersisted {
                    records: 1,
                    at: InstantMillis(1),
                    state_not_saved: false,
                },
                Some(PersistenceState::Durable),
            ),
            (
                EmbeddedPersistenceDiagnostic::BatchPersisted {
                    records: 1,
                    at: InstantMillis(1),
                    state_not_saved: true,
                },
                Some(PersistenceState::Deferred),
            ),
            (
                EmbeddedPersistenceDiagnostic::CompactionStarted {
                    at: InstantMillis(1),
                    next_allowed_at: InstantMillis(2),
                },
                None,
            ),
            (
                EmbeddedPersistenceDiagnostic::CompactionCompleted {
                    records: 1,
                    at: InstantMillis(1),
                    state_not_saved: false,
                },
                Some(PersistenceState::Durable),
            ),
            (
                EmbeddedPersistenceDiagnostic::CompactionCompleted {
                    records: 1,
                    at: InstantMillis(1),
                    state_not_saved: true,
                },
                Some(PersistenceState::Deferred),
            ),
            (
                EmbeddedPersistenceDiagnostic::DurabilityDeferred {
                    target: EmbeddedPersistenceTarget::Routes,
                    until: InstantMillis(1),
                },
                Some(PersistenceState::Deferred),
            ),
            (
                EmbeddedPersistenceDiagnostic::WriteFailed {
                    failure: EmbeddedPersistenceFailure::Flash,
                    retry_at: InstantMillis(1),
                },
                Some(PersistenceState::Failed),
            ),
        ];

        for (diagnostic, expected) in cases {
            assert_eq!(
                PersistenceState::from_embedded_diagnostic(&diagnostic),
                expected
            );
        }
    }
}
