use std::fmt;
use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::error::{AppError, ErrorCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    ResolvingRelease,
    ValidatingManifest,
    Downloading,
    VerifyingArtifacts,
    PublishingCache,
    Ready,
    RequestingPort,
    Connecting,
    VerifyingTarget,
    Writing,
    VerifyingFlash,
    Resetting,
    Monitor,
    Building,
    BuildingEmbeddedSite,
    ArtifactReady,
    Complete,
    Failed,
}

impl Phase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ResolvingRelease => "resolving_release",
            Self::ValidatingManifest => "validating_manifest",
            Self::Downloading => "downloading",
            Self::VerifyingArtifacts => "verifying_artifacts",
            Self::PublishingCache => "publishing_cache",
            Self::Ready => "ready",
            Self::RequestingPort => "requesting_port",
            Self::Connecting => "connecting",
            Self::VerifyingTarget => "verifying_target",
            Self::Writing => "writing",
            Self::VerifyingFlash => "verifying_flash",
            Self::Resetting => "resetting",
            Self::Monitor => "monitor",
            Self::Building => "building",
            Self::BuildingEmbeddedSite => "building_embedded_site",
            Self::ArtifactReady => "artifact_ready",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for Phase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EventKind {
    Phase,
    Progress,
    Success,
    Error,
}

impl EventKind {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Error)
    }
}

#[derive(Serialize)]
struct Event<'a> {
    schema: u8,
    event: EventKind,
    phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    board: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<ErrorCode>,
}

#[derive(Clone, Copy)]
enum OutputMode {
    Human,
    JsonLines,
}

#[derive(Clone, Copy)]
pub(crate) struct Reporter {
    output_mode: OutputMode,
}

impl Reporter {
    pub(crate) const fn human() -> Self {
        Self {
            output_mode: OutputMode::Human,
        }
    }

    pub(crate) const fn json_lines() -> Self {
        Self {
            output_mode: OutputMode::JsonLines,
        }
    }

    pub(crate) fn phase(self, phase: Phase, board: Option<&str>, message: &str) {
        if matches!(self.output_mode, OutputMode::JsonLines) {
            self.emit(Event {
                schema: 1,
                event: EventKind::Phase,
                phase,
                board,
                current: None,
                total: None,
                message: Some(message),
                error_code: None,
            });
        } else {
            println!("{message}");
        }
    }

    pub(crate) fn progress(self, phase: Phase, board: Option<&str>, current: u64, total: u64) {
        if matches!(self.output_mode, OutputMode::JsonLines) {
            self.emit(Event {
                schema: 1,
                event: EventKind::Progress,
                phase,
                board,
                current: Some(current),
                total: Some(total),
                message: None,
                error_code: None,
            });
        } else if let Some(percent) = current.saturating_mul(100).checked_div(total) {
            print!("\r  {phase:<18} {percent:>3}%");
            let _ = io::stdout().flush();
            if current >= total {
                println!();
            }
        }
    }

    pub(crate) fn success(self, board: &str, message: &str) {
        self.emit_success(Some(board), message);
    }

    pub(crate) fn operation_success(self, message: &str) {
        self.emit_success(None, message);
    }

    fn emit_success(self, board: Option<&str>, message: &str) {
        if matches!(self.output_mode, OutputMode::JsonLines) {
            self.emit(Event {
                schema: 1,
                event: EventKind::Success,
                phase: Phase::Complete,
                board,
                current: None,
                total: None,
                message: Some(message),
                error_code: None,
            });
        } else {
            println!("{message}");
        }
    }

    pub(crate) fn error(self, error: &AppError) {
        if matches!(self.output_mode, OutputMode::JsonLines) {
            self.emit(Event {
                schema: 1,
                event: EventKind::Error,
                phase: Phase::Failed,
                board: None,
                current: None,
                total: None,
                message: Some(&error.to_string()),
                error_code: Some(error.error_code()),
            });
        } else {
            eprintln!("error: {error}");
            eprintln!("recovery: {}", error.recovery());
        }
    }

    fn emit(self, event: Event<'_>) {
        debug_assert_eq!(
            event.event.is_terminal(),
            matches!(event.phase, Phase::Complete | Phase::Failed)
        );
        match serde_json::to_string(&event) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!("error: could not encode JSON event: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, EventKind, Phase};
    use crate::error::{AppError, ErrorCode};

    #[test]
    fn progress_event_schema_is_a_single_stable_json_line() {
        let encoded = serde_json::to_string(&Event {
            schema: 1,
            event: EventKind::Progress,
            phase: Phase::Writing,
            board: Some("heltec-v4"),
            current: Some(1024),
            total: Some(4096),
            message: None,
            error_code: None,
        })
        .expect("event serializes");
        assert_eq!(
            encoded,
            r#"{"schema":1,"event":"progress","phase":"writing","board":"heltec-v4","current":1024,"total":4096}"#
        );
        assert!(!encoded.contains('\n'));
    }

    #[test]
    fn terminal_events_are_explicit_and_typed() {
        assert!(!EventKind::Phase.is_terminal());
        assert!(!EventKind::Progress.is_terminal());
        assert!(EventKind::Success.is_terminal());
        assert!(EventKind::Error.is_terminal());

        let cancellation = AppError::Cancelled;
        let encoded = serde_json::to_string(&Event {
            schema: 1,
            event: EventKind::Error,
            phase: Phase::Failed,
            board: None,
            current: None,
            total: None,
            message: Some(&cancellation.to_string()),
            error_code: Some(cancellation.error_code()),
        })
        .expect("event serializes");
        assert_eq!(
            encoded,
            r#"{"schema":1,"event":"error","phase":"failed","message":"operation cancelled; no success was reported","error_code":"cancelled"}"#
        );

        let operation_success = serde_json::to_string(&Event {
            schema: 1,
            event: EventKind::Success,
            phase: Phase::Complete,
            board: None,
            current: None,
            total: None,
            message: Some("candidate imported"),
            error_code: None,
        })
        .expect("event serializes");
        assert_eq!(
            operation_success,
            r#"{"schema":1,"event":"success","phase":"complete","message":"candidate imported"}"#
        );
    }

    #[test]
    fn errors_have_no_credential_fields_or_raw_control_characters() {
        let encoded = serde_json::to_string(&Event {
            schema: 1,
            event: EventKind::Error,
            phase: Phase::Failed,
            board: None,
            current: None,
            total: None,
            message: Some("configuration was rejected\nretry"),
            error_code: Some(ErrorCode::Usage),
        })
        .expect("event serializes");
        assert_eq!(
            encoded,
            r#"{"schema":1,"event":"error","phase":"failed","message":"configuration was rejected\nretry","error_code":"usage"}"#
        );
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("ssid"));
        assert!(!encoded.contains('\n'));
        assert!(!encoded.contains('\u{1b}'));
    }
}
