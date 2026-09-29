use std::path::Path;

use super::{range, AttributionError, MapContribution, MapDocument, MapSection};

const HEADER: &str = "Linker script and memory map";

pub(super) fn parse(path: &Path, contents: &str) -> Result<MapDocument, AttributionError> {
    let Some((_, body)) = contents.split_once(HEADER) else {
        return Err(AttributionError::Unrecognized {
            path: path.to_path_buf(),
            flavor: "GNU ld",
        });
    };
    let mut sections = Vec::<MapSection>::new();
    let mut pending_input = None::<String>;
    let mut current_section = None::<usize>;
    for line in body.lines() {
        if !line.starts_with(char::is_whitespace) {
            pending_input = None;
            current_section = None;
            if let Some(section) = output_section(path, line)? {
                sections.push(section);
                current_section = Some(sections.len() - 1);
            }
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.is_empty() {
            continue;
        }
        if fields[0].starts_with('.') {
            if let Some(contribution) = inline_contribution(path, &fields)? {
                if let Some(section) = current_section.and_then(|index| sections.get_mut(index)) {
                    section.push(contribution);
                }
                pending_input = None;
            } else if fields.len() == 1 {
                pending_input = Some(fields[0].to_string());
            }
            continue;
        }
        if let Some(input_section) = pending_input.take() {
            if let Some(contribution) = split_contribution(path, &input_section, &fields)? {
                if let Some(section) = current_section.and_then(|index| sections.get_mut(index)) {
                    section.push(contribution);
                }
            }
        }
    }
    if sections.is_empty() {
        Err(AttributionError::Unrecognized {
            path: path.to_path_buf(),
            flavor: "GNU ld",
        })
    } else {
        Ok(MapDocument::new(sections))
    }
}

fn output_section(path: &Path, line: &str) -> Result<Option<MapSection>, AttributionError> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 3 || !fields[0].starts_with('.') {
        return Ok(None);
    }
    let (Some(start), Some(bytes)) = (hexadecimal(fields[1]), hexadecimal(fields[2])) else {
        return Ok(None);
    };
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(MapSection::new(
        fields[0].to_string(),
        range(path, fields[0], start, bytes)?,
    )))
}

fn inline_contribution(
    path: &Path,
    fields: &[&str],
) -> Result<Option<MapContribution>, AttributionError> {
    if fields.len() < 4 {
        return Ok(None);
    }
    contribution(path, fields[0], fields[1], fields[2])
}

fn split_contribution(
    path: &Path,
    input_section: &str,
    fields: &[&str],
) -> Result<Option<MapContribution>, AttributionError> {
    if fields.len() < 3 {
        return Ok(None);
    }
    contribution(path, input_section, fields[0], fields[1])
}

fn contribution(
    path: &Path,
    input_section: &str,
    start: &str,
    bytes: &str,
) -> Result<Option<MapContribution>, AttributionError> {
    let (Some(start), Some(bytes)) = (hexadecimal(start), hexadecimal(bytes)) else {
        return Ok(None);
    };
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(MapContribution::new(
        input_section.to_string(),
        range(path, input_section, start, bytes)?,
    )))
}

fn hexadecimal(value: &str) -> Option<u64> {
    value
        .strip_prefix("0x")
        .and_then(|value| u64::from_str_radix(value, 16).ok())
}
