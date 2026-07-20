use crate::configobj;
use crate::diagnostic::{ConfigDiagnostic, ConfigDiagnosticCode, ConfigErrors, ConfigReport};

use super::interpret::interpret;
use super::types::ReferenceConfig;
use super::validation::{legacy_diagnostic, validate, ValidationResult};

pub fn parse(input: &str) -> Result<ReferenceConfig, ConfigErrors> {
    parse_named("config", input).map(|report| report.value)
}

pub fn parse_named(
    source: impl Into<String>,
    input: &str,
) -> Result<ConfigReport<ReferenceConfig>, ConfigErrors> {
    let source = source.into();
    let parsed = match configobj::parse_located(input) {
        Ok(parsed) => parsed,
        Err(error) => {
            let line = error.line();
            return Err(ConfigErrors::new(vec![ConfigDiagnostic::new(
                ConfigDiagnosticCode::Syntax,
                source,
                line,
                "<document>",
                None,
                error.to_string(),
                Some(
                    "stock ConfigObj syntax with section headers and key = value entries"
                        .to_string(),
                ),
                format!("correct the syntax on line {line}"),
            )]));
        }
    };
    let warnings = match validate(&source, &parsed.root, &parsed.locations) {
        ValidationResult::Valid { warnings } => warnings.into_inner(),
        ValidationResult::Invalid { errors, warnings } => {
            return Err(ConfigErrors::new((*errors).with_warnings(warnings)));
        }
    };
    let value = interpret(&parsed.root).map_err(|error| {
        ConfigErrors::new(vec![legacy_diagnostic(&source, &parsed.locations, error)])
    })?;
    Ok(ConfigReport {
        value,
        warnings,
        source,
        locations: parsed.locations,
    })
}
