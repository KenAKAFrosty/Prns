use std::io::{self, Write};

use serde::Serialize;

use crate::error::AppError;

#[derive(Clone, Copy)]
pub(crate) struct Reporter {
    json: bool,
}

#[derive(Serialize)]
struct Event<'a> {
    schema: u8,
    event: &'a str,
    phase: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    board: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<&'a str>,
}

impl Reporter {
    pub(crate) const fn new(json: bool) -> Self {
        Self { json }
    }

    pub(crate) fn phase(self, phase: &str, board: Option<&str>, message: &str) {
        if self.json {
            self.emit(Event {
                schema: 1,
                event: "phase",
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

    pub(crate) fn progress(self, phase: &str, board: Option<&str>, current: u64, total: u64) {
        if self.json {
            self.emit(Event {
                schema: 1,
                event: "progress",
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
        if self.json {
            self.emit(Event {
                schema: 1,
                event: "success",
                phase: "complete",
                board: Some(board),
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
        if self.json {
            self.emit(Event {
                schema: 1,
                event: "error",
                phase: "failed",
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
        match serde_json::to_string(&event) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!("error: could not encode JSON event: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Event;

    #[test]
    fn progress_event_schema_is_a_single_stable_json_line() {
        let encoded = serde_json::to_string(&Event {
            schema: 1,
            event: "progress",
            phase: "writing",
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
    fn errors_do_not_have_a_credential_field() {
        let encoded = serde_json::to_string(&Event {
            schema: 1,
            event: "error",
            phase: "failed",
            board: None,
            current: None,
            total: None,
            message: Some("configuration was rejected"),
            error_code: Some("usage"),
        })
        .expect("event serializes");
        assert_eq!(
            encoded,
            r#"{"schema":1,"event":"error","phase":"failed","message":"configuration was rejected","error_code":"usage"}"#
        );
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("ssid"));
    }
}
