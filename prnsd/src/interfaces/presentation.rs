use std::fmt::Write;
use std::path::Path;

use prns_config::editing::{ConfiguredInterface, SecretDisplay};
use prns_config::{
    ConfigDiagnostic, ConfigDiagnosticCode, ConfigErrors, ConfigReport, InterfaceKind,
};

use crate::terminal::{self, ACCENT, ACCENT_STRONG, ERROR, MUTED, PROMPT, WARNING};

pub(super) struct Presentation {
    styled: bool,
}

impl Presentation {
    pub(super) const fn new(styled: bool) -> Self {
        Self { styled }
    }

    pub(super) fn main_screen(
        &self,
        path: &Path,
        interfaces: &[ConfiguredInterface],
        validation: &ValidationState,
        runtime: RuntimeStatus,
        apply: ApplyStatus,
    ) -> String {
        let mut output = String::new();
        let _ = writeln!(output);
        let _ = writeln!(
            output,
            "{}",
            terminal::paint("Interface configuration", ACCENT, self.styled)
        );
        let _ = writeln!(
            output,
            "{}",
            terminal::paint(path.display().to_string(), MUTED, self.styled)
        );
        let _ = writeln!(output);
        let _ = writeln!(output, "{}", self.validation_status(validation));
        let _ = writeln!(output, "{}", self.runtime_status(runtime, apply));
        let _ = writeln!(output);
        if interfaces.is_empty() {
            let _ = writeln!(output, "  No interfaces are configured.");
        } else {
            for (index, interface) in interfaces.iter().enumerate() {
                let _ = writeln!(
                    output,
                    "  {}  {}",
                    terminal::paint((index + 1).to_string(), PROMPT, self.styled),
                    terminal::bold(interface.name().to_string(), self.styled)
                );
                let _ = writeln!(output, "     {}", self.interface_summary(interface));
                let _ = writeln!(output);
            }
        }
        let _ = writeln!(output, "Choose an interface number to manage.");
        let _ = writeln!(output);
        let _ = writeln!(
            output,
            "{}",
            terminal::paint(
                "[A] Add       [V] Validate      [R] Repair",
                ACCENT,
                self.styled
            )
        );
        let _ = writeln!(
            output,
            "{}",
            terminal::paint("[P] Apply     [Q] Quit", ACCENT, self.styled)
        );
        output
    }

    pub(super) fn validation(
        &self,
        path: &Path,
        validation: &ValidationState,
        details: bool,
        secrets: SecretDisplay,
    ) -> String {
        let mut output = String::new();
        let _ = writeln!(output);
        let _ = writeln!(
            output,
            "{}",
            terminal::paint("Validation", ACCENT, self.styled)
        );
        let _ = writeln!(
            output,
            "{}",
            terminal::paint(path.display().to_string(), MUTED, self.styled)
        );
        let _ = writeln!(output);
        let _ = writeln!(output, "{}", self.validation_status(validation));
        if validation.diagnostics().is_empty() {
            return output;
        }
        let _ = writeln!(output);
        for group in grouped(validation.diagnostics()) {
            let _ = writeln!(output, "  {}", terminal::bold(group.name, self.styled));
            for diagnostic in group.diagnostics {
                let color = if diagnostic.code() == ConfigDiagnosticCode::PersistedRuntimeMetadata {
                    ACCENT_STRONG
                } else if diagnostic.severity() == prns_config::ConfigSeverity::Error {
                    ERROR
                } else {
                    WARNING
                };
                let key = diagnostic
                    .path()
                    .rsplit(" > ")
                    .next()
                    .unwrap_or(diagnostic.path());
                let _ = writeln!(
                    output,
                    "    {} {}: {}",
                    terminal::paint("•", color, self.styled),
                    key,
                    diagnostic.message()
                );
                if details {
                    let _ = writeln!(
                        output,
                        "      {}",
                        terminal::paint(
                            diagnostic.display_with(secrets).to_string(),
                            MUTED,
                            self.styled
                        )
                    );
                }
            }
            let _ = writeln!(output);
        }
        output
    }

    pub(super) fn repair_summary(
        &self,
        path: &Path,
        diagnostics: &[ConfigDiagnostic],
        details: bool,
        secrets: SecretDisplay,
    ) -> String {
        let state = ValidationState::from_diagnostics(diagnostics.to_vec());
        self.validation(path, &state, details, secrets)
            .replacen("Validation", "Repair", 1)
    }

    pub(super) fn interface_header(&self, interface: &ConfiguredInterface) -> String {
        let mut output = String::new();
        let _ = writeln!(output);
        let _ = writeln!(
            output,
            "{}",
            terminal::paint(interface.name().to_string(), ACCENT, self.styled)
        );
        let _ = writeln!(output, "{}", self.interface_summary(interface));
        output
    }

    pub(super) fn prompt(&self, label: &str) -> String {
        terminal::paint(label, PROMPT, self.styled)
    }

    pub(super) fn success(&self, text: impl AsRef<str>) -> String {
        terminal::paint(text, ACCENT_STRONG, self.styled)
    }

    pub(super) fn warning(&self, text: impl AsRef<str>) -> String {
        terminal::paint(text, WARNING, self.styled)
    }

    pub(super) fn error(&self, text: impl AsRef<str>) -> String {
        terminal::paint(text, ERROR, self.styled)
    }

    pub(super) fn muted(&self, text: impl AsRef<str>) -> String {
        terminal::paint(text, MUTED, self.styled)
    }

    fn validation_status(&self, validation: &ValidationState) -> String {
        let errors = validation.error_count();
        let warnings = validation.warning_count();
        let cleanup = validation.cleanup_count();
        if errors > 0 {
            let suffix = count_summary(errors, warnings, cleanup);
            return terminal::paint(
                format!("✗ Configuration invalid · {suffix}"),
                ERROR,
                self.styled,
            );
        }
        if warnings > 0 {
            let suffix = if cleanup == warnings {
                format!("{cleanup} cleanup {}", plural(cleanup, "item", "items"))
            } else {
                count_summary(errors, warnings, cleanup)
            };
            return terminal::paint(
                format!("✓ Configuration valid · {suffix}"),
                WARNING,
                self.styled,
            );
        }
        terminal::paint("✓ Configuration valid", ACCENT_STRONG, self.styled)
    }

    fn runtime_status(&self, runtime: RuntimeStatus, apply: ApplyStatus) -> String {
        match (runtime, apply) {
            (RuntimeStatus::Running, ApplyStatus::Pending) => terminal::paint(
                "● Managed daemon running · saved changes not applied",
                WARNING,
                self.styled,
            ),
            (RuntimeStatus::Running, ApplyStatus::Current) => terminal::paint(
                "● Managed daemon running · saved interface plan applied",
                ACCENT_STRONG,
                self.styled,
            ),
            (RuntimeStatus::Running, ApplyStatus::Unknown) => {
                terminal::paint("● Managed daemon running", ACCENT_STRONG, self.styled)
            }
            (RuntimeStatus::Starting, _) => {
                terminal::paint("◐ Managed daemon starting", WARNING, self.styled)
            }
            (RuntimeStatus::Stopped, _) => {
                terminal::paint("○ Managed daemon stopped", MUTED, self.styled)
            }
            (RuntimeStatus::Unavailable, _) => {
                terminal::paint("! Managed daemon status unavailable", WARNING, self.styled)
            }
        }
    }

    fn interface_summary(&self, interface: &ConfiguredInterface) -> String {
        let kind = interface
            .kind()
            .map(friendly_kind)
            .unwrap_or("Unknown type");
        let configured = interface.configured_type().unwrap_or("missing type");
        let state = match interface.enabled() {
            Some(true) => terminal::paint("Enabled", ACCENT_STRONG, self.styled),
            Some(false) => terminal::paint("Disabled", MUTED, self.styled),
            None => terminal::paint("Invalid state", ERROR, self.styled),
        };
        format!("{kind} · {configured} · {state}")
    }
}

#[derive(Debug, Clone)]
pub(super) enum ValidationState {
    Valid(Vec<ConfigDiagnostic>),
    Invalid(Vec<ConfigDiagnostic>),
}

impl ValidationState {
    pub(super) fn from_result<T>(result: Result<ConfigReport<T>, ConfigErrors>) -> Self {
        match result {
            Ok(report) => Self::Valid(report.warnings),
            Err(errors) => Self::Invalid(errors.diagnostics().to_vec()),
        }
    }

    pub(super) fn from_diagnostics(diagnostics: Vec<ConfigDiagnostic>) -> Self {
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity() == prns_config::ConfigSeverity::Error)
        {
            Self::Invalid(diagnostics)
        } else {
            Self::Valid(diagnostics)
        }
    }

    pub(super) fn diagnostics(&self) -> &[ConfigDiagnostic] {
        match self {
            Self::Valid(diagnostics) | Self::Invalid(diagnostics) => diagnostics,
        }
    }

    pub(super) fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }

    fn error_count(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity() == prns_config::ConfigSeverity::Error)
            .count()
    }

    fn warning_count(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity() == prns_config::ConfigSeverity::Warning)
            .count()
    }

    fn cleanup_count(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == ConfigDiagnosticCode::PersistedRuntimeMetadata
            })
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeStatus {
    Running,
    Starting,
    Stopped,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApplyStatus {
    Unknown,
    Pending,
    Current,
}

struct DiagnosticGroup<'a> {
    name: String,
    diagnostics: Vec<&'a ConfigDiagnostic>,
}

fn grouped(diagnostics: &[ConfigDiagnostic]) -> Vec<DiagnosticGroup<'_>> {
    let mut groups = Vec::<DiagnosticGroup<'_>>::new();
    for diagnostic in diagnostics {
        let name = interface_name(diagnostic.path()).unwrap_or_else(|| "Configuration".to_string());
        match groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.diagnostics.push(diagnostic),
            None => groups.push(DiagnosticGroup {
                name,
                diagnostics: vec![diagnostic],
            }),
        }
    }
    groups
}

fn interface_name(path: &str) -> Option<String> {
    let start = path.find("[[")? + 2;
    let rest = &path[start..];
    let end = rest.find("]]")?;
    Some(rest[..end].trim().to_string())
}

pub(super) fn friendly_kind(kind: InterfaceKind) -> &'static str {
    match kind {
        InterfaceKind::Auto => "Auto / Wi-Fi",
        InterfaceKind::TcpClient => "TCP client",
        InterfaceKind::TcpServer => "TCP server",
        InterfaceKind::Udp => "UDP",
        InterfaceKind::Serial => "Serial",
        InterfaceKind::Kiss => "KISS",
        InterfaceKind::Ax25Kiss => "AX.25 KISS",
        InterfaceKind::Rnode => "RNode",
        InterfaceKind::RnodeMulti => "RNode Multi",
        InterfaceKind::Pipe => "Pipe",
        InterfaceKind::Backbone => "Backbone server",
        InterfaceKind::BackboneClient => "Backbone client",
        InterfaceKind::I2p => "I2P",
        InterfaceKind::Weave => "Weave",
        InterfaceKind::PrnsUsbAuto => "USB Auto",
        InterfaceKind::PrnsBluetoothAuto => "Bluetooth / BLE Auto",
        InterfaceKind::PrnsWebSocketClient => "WebSocket client",
        InterfaceKind::PrnsWebSocketServer => "WebSocket server",
    }
}

fn count_summary(errors: usize, warnings: usize, cleanup: usize) -> String {
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(format!("{errors} {}", plural(errors, "error", "errors")));
    }
    if warnings > cleanup {
        let count = warnings - cleanup;
        parts.push(format!("{count} {}", plural(count, "warning", "warnings")));
    }
    if cleanup > 0 {
        parts.push(format!(
            "{cleanup} cleanup {}",
            plural(cleanup, "item", "items")
        ));
    }
    parts.join(" · ")
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use prns_config::configobj::ConfigDocument;
    use prns_config::parse_and_plan_named;

    use super::{ApplyStatus, Presentation, RuntimeStatus, ValidationState};

    const SOURCE: &str = "[interfaces]\n[[Default Interface]]\ntype = AutoInterface\ninterface_enabled = Yes\nname = Default Interface\nselected_interface_mode = 1\nconfigured_bitrate = None\n";

    #[test]
    fn plain_main_screen_has_hierarchy_without_escape_sequences() {
        let document = ConfigDocument::parse(SOURCE).unwrap_or_else(|error| panic!("{error}"));
        let validation = ValidationState::from_result(parse_and_plan_named("config", SOURCE));
        let rendered = Presentation::new(false).main_screen(
            Path::new("/tmp/config"),
            &document.interfaces(),
            &validation,
            RuntimeStatus::Running,
            ApplyStatus::Pending,
        );

        assert!(rendered.contains("Interface configuration\n/tmp/config"));
        assert!(rendered.contains("Configuration valid · 3 cleanup items"));
        assert!(rendered.contains("Auto / Wi-Fi · AutoInterface · Enabled"));
        assert!(rendered.contains("saved changes not applied"));
        assert!(!rendered.contains("\x1b["));
    }

    #[test]
    fn styled_validation_groups_cleanup_under_the_interface() {
        let validation = ValidationState::from_result(parse_and_plan_named("config", SOURCE));
        let rendered = Presentation::new(true).validation(
            Path::new("/tmp/config"),
            &validation,
            false,
            prns_config::SecretDisplay::Redacted,
        );

        assert!(rendered.contains("\x1b["));
        assert!(rendered.contains("Default Interface"));
        assert!(rendered.contains("selected_interface_mode"));
        assert!(!rendered.contains("warning[persisted_runtime_metadata]"));
    }
}
