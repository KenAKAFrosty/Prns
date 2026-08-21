use std::fmt;

use super::contract::{
    self, BridgeErrorCode, BridgeOperation, BridgePhase, ErrorPolicy, PartPolicy, ProgressPolicy,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProtocolViolation {
    Schema(u8),
    ErrorForbidden(BridgePhase),
    ErrorRequired(BridgePhase),
    CancelledCode,
    RecoveryMessage(BridgePhase),
    ProgressPair,
    ProgressForbidden(BridgePhase),
    ProgressRequired(BridgePhase),
    ProgressIncomplete(BridgePhase),
    ProgressBounds,
    PartPair,
    PartForbidden(BridgePhase),
    PartRequired(BridgePhase),
    PartBounds,
    FieldRequired(BridgePhase, &'static str),
    FieldForbidden(BridgePhase, &'static str),
    Initial(BridgeOperation, BridgePhase),
    Transition(BridgePhase, BridgePhase),
    AfterTerminal,
    TotalChanged,
    ProgressRegressed,
}

impl fmt::Display for ProtocolViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(schema) => write!(formatter, "unsupported bridge event schema {schema}"),
            Self::ErrorForbidden(phase) => {
                write!(formatter, "bridge phase {phase} cannot carry an error code")
            }
            Self::ErrorRequired(phase) => {
                write!(
                    formatter,
                    "bridge phase {phase} requires a failure error code"
                )
            }
            Self::CancelledCode => {
                formatter.write_str("bridge cancellation requires the cancelled error code")
            }
            Self::RecoveryMessage(phase) => {
                write!(
                    formatter,
                    "bridge phase {phase} requires a recovery message"
                )
            }
            Self::ProgressPair => {
                formatter.write_str("bridge progress requires current and total bytes together")
            }
            Self::ProgressForbidden(phase) => {
                write!(formatter, "bridge phase {phase} cannot carry byte progress")
            }
            Self::ProgressRequired(phase) => {
                write!(formatter, "bridge phase {phase} requires byte progress")
            }
            Self::ProgressIncomplete(phase) => {
                write!(
                    formatter,
                    "bridge phase {phase} requires complete byte progress"
                )
            }
            Self::ProgressBounds => {
                formatter.write_str("bridge byte progress is outside its declared total")
            }
            Self::PartPair => formatter.write_str(
                "bridge part progress requires part, part index, and part count together",
            ),
            Self::PartForbidden(phase) => {
                write!(formatter, "bridge phase {phase} cannot carry part progress")
            }
            Self::PartRequired(phase) => {
                write!(formatter, "bridge phase {phase} requires part progress")
            }
            Self::PartBounds => {
                formatter.write_str("bridge part progress is outside its declared part count")
            }
            Self::FieldRequired(phase, field) => {
                write!(formatter, "bridge phase {phase} requires {field}")
            }
            Self::FieldForbidden(phase, field) => {
                write!(formatter, "bridge phase {phase} cannot carry {field}")
            }
            Self::Initial(operation, phase) => write!(
                formatter,
                "bridge operation {} cannot begin with {phase}",
                operation.wire()
            ),
            Self::Transition(previous, next) => {
                write!(
                    formatter,
                    "bridge transition {previous} -> {next} is not permitted"
                )
            }
            Self::AfterTerminal => {
                formatter.write_str("bridge operation emitted after its terminal event")
            }
            Self::TotalChanged => {
                formatter.write_str("bridge progress total changed during one operation")
            }
            Self::ProgressRegressed => {
                formatter.write_str("bridge progress moved backwards during one operation")
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct EventFacts<'a> {
    pub(super) schema: u8,
    pub(super) phase: BridgePhase,
    pub(super) code: Option<BridgeErrorCode>,
    pub(super) message: Option<&'a str>,
    pub(super) current: Option<u64>,
    pub(super) total: Option<u64>,
    pub(super) part: Option<&'a str>,
    pub(super) part_index: Option<usize>,
    pub(super) part_count: Option<usize>,
    pub(super) detected_chip: Option<&'a str>,
    pub(super) bytes: Option<u64>,
}

pub(super) struct EventSequence {
    operation: BridgeOperation,
    phase: Option<BridgePhase>,
    terminal: bool,
    current: Option<u64>,
    total: Option<u64>,
}

impl EventSequence {
    pub(super) fn new(operation: BridgeOperation) -> Self {
        let _ = contract::operation(operation);
        Self {
            operation,
            phase: None,
            terminal: false,
            current: None,
            total: None,
        }
    }

    pub(super) fn accept(&mut self, event: EventFacts<'_>) -> Result<(), ProtocolViolation> {
        validate_event(event)?;
        if self.terminal {
            return Err(ProtocolViolation::AfterTerminal);
        }

        let allowed = self.phase.map_or_else(
            || contract::operation(self.operation).initial(),
            |phase| contract::phase(phase).next(),
        );
        if !allowed.contains(&event.phase) {
            return Err(match self.phase {
                Some(previous) => ProtocolViolation::Transition(previous, event.phase),
                None => ProtocolViolation::Initial(self.operation, event.phase),
            });
        }

        if let (Some(current), Some(total)) = (event.current, event.total) {
            if self.total.is_some_and(|expected| total != expected) {
                return Err(ProtocolViolation::TotalChanged);
            }
            if self.current.is_some_and(|previous| current < previous) {
                return Err(ProtocolViolation::ProgressRegressed);
            }
            self.current = Some(current);
            self.total = Some(total);
        }

        self.phase = Some(event.phase);
        self.terminal = contract::phase(event.phase).terminal();
        Ok(())
    }
}

fn validate_event(event: EventFacts<'_>) -> Result<(), ProtocolViolation> {
    if event.schema != contract::schema() {
        return Err(ProtocolViolation::Schema(event.schema));
    }
    let definition = contract::phase(event.phase);

    match definition.error_policy() {
        ErrorPolicy::Forbidden if event.code.is_some() => {
            return Err(ProtocolViolation::ErrorForbidden(event.phase))
        }
        ErrorPolicy::Failure
            if event.code.is_none() || event.code == Some(BridgeErrorCode::Cancelled) =>
        {
            return Err(ProtocolViolation::ErrorRequired(event.phase))
        }
        ErrorPolicy::Cancelled if event.code != Some(BridgeErrorCode::Cancelled) => {
            return Err(ProtocolViolation::CancelledCode)
        }
        _ => {}
    }
    if definition.error_policy() != ErrorPolicy::Forbidden
        && event
            .message
            .is_none_or(|message| message.trim().is_empty())
    {
        return Err(ProtocolViolation::RecoveryMessage(event.phase));
    }

    let has_current = event.current.is_some();
    let has_total = event.total.is_some();
    if has_current != has_total {
        return Err(ProtocolViolation::ProgressPair);
    }
    match definition.progress_policy() {
        ProgressPolicy::Forbidden if has_current => {
            return Err(ProtocolViolation::ProgressForbidden(event.phase))
        }
        ProgressPolicy::Required | ProgressPolicy::Complete if !has_current => {
            return Err(ProtocolViolation::ProgressRequired(event.phase))
        }
        _ => {}
    }
    if let (Some(current), Some(total)) = (event.current, event.total) {
        if total < contract::minimum_progress_total() || current > total {
            return Err(ProtocolViolation::ProgressBounds);
        }
        if definition.progress_policy() == ProgressPolicy::Complete && current != total {
            return Err(ProtocolViolation::ProgressIncomplete(event.phase));
        }
    }

    let part_fields = [
        event.part.is_some(),
        event.part_index.is_some(),
        event.part_count.is_some(),
    ];
    let present_part_fields = part_fields.into_iter().filter(|present| *present).count();
    if present_part_fields != 0 && present_part_fields != part_fields.len() {
        return Err(ProtocolViolation::PartPair);
    }
    match definition.part_policy() {
        PartPolicy::Forbidden if present_part_fields != 0 => {
            return Err(ProtocolViolation::PartForbidden(event.phase))
        }
        PartPolicy::Required if present_part_fields != part_fields.len() => {
            return Err(ProtocolViolation::PartRequired(event.phase))
        }
        _ => {}
    }
    if let (Some(part), Some(index), Some(count)) = (event.part, event.part_index, event.part_count)
    {
        if part.is_empty() || count == 0 || index >= count {
            return Err(ProtocolViolation::PartBounds);
        }
    }

    validate_exclusive_field(
        event.phase,
        "detectedChip",
        event.detected_chip.is_some(),
        definition.requires_detected_chip(),
    )?;
    if event
        .detected_chip
        .is_some_and(|chip| chip.trim().is_empty())
    {
        return Err(ProtocolViolation::FieldRequired(
            event.phase,
            "non-empty detectedChip",
        ));
    }
    validate_exclusive_field(
        event.phase,
        "bytes",
        event.bytes.is_some(),
        definition.requires_bytes(),
    )?;
    Ok(())
}

fn validate_exclusive_field(
    phase: BridgePhase,
    field: &'static str,
    present: bool,
    required: bool,
) -> Result<(), ProtocolViolation> {
    match (present, required) {
        (false, true) => Err(ProtocolViolation::FieldRequired(phase, field)),
        (true, false) => Err(ProtocolViolation::FieldForbidden(phase, field)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(phase: BridgePhase) -> EventFacts<'static> {
        EventFacts {
            schema: contract::schema(),
            phase,
            code: None,
            message: None,
            current: None,
            total: None,
            part: None,
            part_index: None,
            part_count: None,
            detected_chip: None,
            bytes: None,
        }
    }

    #[test]
    fn phase_semantics_reject_invalid_error_and_progress_shapes() {
        assert_eq!(
            validate_event(EventFacts {
                code: Some(BridgeErrorCode::ConnectionFailure),
                ..event(BridgePhase::Connecting)
            }),
            Err(ProtocolViolation::ErrorForbidden(BridgePhase::Connecting))
        );
        assert_eq!(
            validate_event(event(BridgePhase::Failed)),
            Err(ProtocolViolation::ErrorRequired(BridgePhase::Failed))
        );
        assert_eq!(
            validate_event(EventFacts {
                code: Some(BridgeErrorCode::Cancelled),
                ..event(BridgePhase::Cancelled)
            }),
            Err(ProtocolViolation::RecoveryMessage(BridgePhase::Cancelled))
        );
        assert_eq!(
            validate_event(EventFacts {
                current: Some(1),
                total: Some(2),
                bytes: Some(2),
                ..event(BridgePhase::Ready)
            }),
            Err(ProtocolViolation::ProgressIncomplete(BridgePhase::Ready))
        );
        assert_eq!(
            validate_event(EventFacts {
                current: Some(0),
                total: Some(0),
                ..event(BridgePhase::Writing)
            }),
            Err(ProtocolViolation::ProgressBounds)
        );
    }

    #[test]
    fn sequences_enforce_transitions_progress_and_one_terminal_event() {
        let mut preparation = EventSequence::new(BridgeOperation::Preparation);
        preparation
            .accept(event(BridgePhase::ValidatingManifest))
            .expect("valid preparation entry");
        preparation
            .accept(EventFacts {
                current: Some(0),
                total: Some(4),
                part: Some("application"),
                part_index: Some(0),
                part_count: Some(1),
                ..event(BridgePhase::Downloading)
            })
            .expect("valid download");
        preparation
            .accept(EventFacts {
                current: Some(4),
                total: Some(4),
                part: Some("application"),
                part_index: Some(0),
                part_count: Some(1),
                ..event(BridgePhase::VerifyingArtifacts)
            })
            .expect("valid verification");
        preparation
            .accept(EventFacts {
                current: Some(4),
                total: Some(4),
                bytes: Some(4),
                ..event(BridgePhase::Ready)
            })
            .expect("valid ready terminal");
        assert_eq!(
            preparation.accept(EventFacts {
                code: Some(BridgeErrorCode::FlashFailed),
                message: Some("recover"),
                ..event(BridgePhase::Failed)
            }),
            Err(ProtocolViolation::AfterTerminal)
        );

        let mut uf2 = EventSequence::new(BridgeOperation::Device);
        uf2.accept(EventFacts {
            current: Some(4),
            total: Some(4),
            ..event(BridgePhase::DownloadRequested)
        })
        .expect("valid UF2 download request terminal");
        assert!(uf2.terminal);

        let mut skipped = EventSequence::new(BridgeOperation::Device);
        skipped
            .accept(event(BridgePhase::RequestingPort))
            .expect("valid device entry");
        assert_eq!(
            skipped.accept(EventFacts {
                current: Some(0),
                total: Some(4),
                ..event(BridgePhase::Writing)
            }),
            Err(ProtocolViolation::Transition(
                BridgePhase::RequestingPort,
                BridgePhase::Writing
            ))
        );

        let mut progress = EventSequence::new(BridgeOperation::Device);
        progress
            .accept(event(BridgePhase::RequestingPort))
            .expect("requesting port");
        progress
            .accept(event(BridgePhase::Connecting))
            .expect("connecting");
        progress
            .accept(EventFacts {
                detected_chip: Some("ESP32-S3"),
                ..event(BridgePhase::VerifyingTarget)
            })
            .expect("target check");
        progress
            .accept(EventFacts {
                current: Some(3),
                total: Some(4),
                ..event(BridgePhase::Writing)
            })
            .expect("write progress");
        assert_eq!(
            progress.accept(EventFacts {
                current: Some(2),
                total: Some(4),
                ..event(BridgePhase::Writing)
            }),
            Err(ProtocolViolation::ProgressRegressed)
        );
        assert_eq!(
            progress.accept(EventFacts {
                current: Some(4),
                total: Some(5),
                ..event(BridgePhase::Writing)
            }),
            Err(ProtocolViolation::TotalChanged)
        );

        let mut managed_nrf = EventSequence::new(BridgeOperation::Device);
        managed_nrf
            .accept(event(BridgePhase::RequestingPort))
            .expect("requesting managed application");
        managed_nrf
            .accept(event(BridgePhase::Connecting))
            .expect("entering Nordic bootloader");
        managed_nrf
            .accept(event(BridgePhase::AwaitingBootloaderPort))
            .expect("bounded bootloader picker continuation");
        managed_nrf
            .accept(EventFacts {
                detected_chip: Some("nRF52840 (2886:0057)"),
                ..event(BridgePhase::VerifyingTarget)
            })
            .expect("exact Nordic target check");

        let mut fresh = EventSequence::new(BridgeOperation::Device);
        fresh
            .accept(event(BridgePhase::RequestingPort))
            .expect("requesting port");
        fresh
            .accept(event(BridgePhase::Connecting))
            .expect("connecting");
        fresh
            .accept(EventFacts {
                detected_chip: Some("ESP32-S3"),
                ..event(BridgePhase::VerifyingTarget)
            })
            .expect("target check");
        fresh
            .accept(event(BridgePhase::Erasing))
            .expect("full-chip erasure");
        assert_eq!(
            fresh.accept(EventFacts {
                code: Some(BridgeErrorCode::Cancelled),
                message: Some("stop"),
                ..event(BridgePhase::Cancelled)
            }),
            Err(ProtocolViolation::Transition(
                BridgePhase::Erasing,
                BridgePhase::Cancelled
            ))
        );
    }
}
