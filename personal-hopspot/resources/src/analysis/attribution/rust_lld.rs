use std::path::Path;

use super::{range, AttributionError, MapContribution, MapDocument, MapSection};

const HEADER: &str = "VMA      LMA     Size Align Out     In      Symbol";

pub(super) fn parse(path: &Path, contents: &str) -> Result<MapDocument, AttributionError> {
    let Some((_, body)) = contents.split_once(HEADER) else {
        return Err(AttributionError::Unrecognized {
            path: path.to_path_buf(),
            flavor: "rust-lld",
        });
    };
    let mut sections = Vec::<MapSection>::new();
    for line in body.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        let Some(start) = hexadecimal(fields[0]) else {
            continue;
        };
        let Some(bytes) = hexadecimal(fields[2]) else {
            continue;
        };
        if bytes == 0 {
            continue;
        }
        let identity = fields[4];
        if let Some((_, input_section)) = identity.rsplit_once(":(") {
            let Some(section) = sections.last_mut() else {
                continue;
            };
            let input_section = input_section.trim_end_matches(')');
            section.push(MapContribution::new(
                input_section.to_string(),
                range(path, input_section, start, bytes)?,
            ));
        } else if identity.starts_with('.') && identity != "." {
            sections.push(MapSection::new(
                identity.to_string(),
                range(path, identity, start, bytes)?,
            ));
        }
    }
    if sections.is_empty() {
        Err(AttributionError::Unrecognized {
            path: path.to_path_buf(),
            flavor: "rust-lld",
        })
    } else {
        Ok(MapDocument::new(sections))
    }
}

fn hexadecimal(value: &str) -> Option<u64> {
    u64::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}
