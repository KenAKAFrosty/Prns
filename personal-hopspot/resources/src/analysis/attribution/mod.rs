mod gnu_ld;
mod rust_lld;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::architecture::LinkerFlavor;
use personal_hopspot_memory::{AddressRange, AddressRangeError};
use rustc_demangle::try_demangle;
use thiserror::Error;

use super::AllocatedSection;

const RANKED_LIMIT: usize = 25;
const SYMBOL_PREFIX_BYTES: usize = 320;
const SYMBOL_SUFFIX_BYTES: usize = 128;

#[derive(Clone, Copy)]
pub(crate) enum AttributionBasis<'a> {
    Complete(&'a [AllocatedSection]),
    Partial,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AttributionAnalysis {
    pub(crate) crate_coverage: AttributionCoverage,
    pub(crate) symbol_coverage: AttributionCoverage,
    pub(crate) largest_crates: Vec<AttributedUsage>,
    pub(crate) largest_symbols: Vec<AttributedUsage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttributionCoverage {
    pub(crate) analyzed_bytes: u64,
    pub(crate) attributed_bytes: u64,
    pub(crate) unclassified_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AttributedUsage {
    pub(crate) name: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Error)]
pub(crate) enum AttributionError {
    #[error("could not read linker map {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("linker map {path} does not contain recognizable {flavor} evidence")]
    Unrecognized { path: PathBuf, flavor: &'static str },
    #[error("linker map {path} contains an invalid range for section {section:?}")]
    InvalidRange { path: PathBuf, section: String },
    #[error("linker map {path} contribution {input:?} escapes output section {output:?}")]
    EscapingContribution {
        path: PathBuf,
        output: String,
        input: String,
    },
    #[error("linker map {path} contains overlapping contributions in section {section:?}")]
    OverlappingContributions { path: PathBuf, section: String },
    #[error("linker map attribution overflowed while accounting {category}")]
    ArithmeticOverflow {
        path: PathBuf,
        category: &'static str,
    },
    #[error("linker map {path} attributes more than the {total} analyzed bytes")]
    AttributionExceedsBasis { path: PathBuf, total: u64 },
}

#[derive(Debug)]
struct MapDocument {
    sections: Vec<MapSection>,
}

#[derive(Debug)]
struct MapSection {
    name: String,
    range: AddressRange,
    contributions: Vec<MapContribution>,
}

#[derive(Debug)]
struct MapContribution {
    input_section: String,
    range: AddressRange,
}

pub(crate) fn analyze_linker_map(
    path: &Path,
    flavor: LinkerFlavor,
    basis: AttributionBasis<'_>,
) -> Result<AttributionAnalysis, AttributionError> {
    let bytes = fs::read(path).map_err(|source| AttributionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let contents = String::from_utf8_lossy(&bytes);
    let document = match flavor {
        LinkerFlavor::RustLld => rust_lld::parse(path, &contents)?,
        LinkerFlavor::GnuLd => gnu_ld::parse(path, &contents)?,
    };
    document.attribute(path, basis)
}

impl MapDocument {
    fn new(sections: Vec<MapSection>) -> Self {
        Self { sections }
    }

    fn attribute(
        mut self,
        path: &Path,
        basis: AttributionBasis<'_>,
    ) -> Result<AttributionAnalysis, AttributionError> {
        let analyzed_bytes = match basis {
            AttributionBasis::Complete(sections) => {
                sections.iter().try_fold(0_u64, |sum, section| {
                    sum.checked_add(section.load_bytes()).ok_or_else(|| {
                        AttributionError::ArithmeticOverflow {
                            path: path.to_path_buf(),
                            category: "analyzed sections",
                        }
                    })
                })?
            }
            AttributionBasis::Partial => self
                .sections
                .iter()
                .filter(|section| probably_loaded(&section.name))
                .try_fold(0_u64, |sum, section| {
                    sum.checked_add(section.range.byte_len()).ok_or_else(|| {
                        AttributionError::ArithmeticOverflow {
                            path: path.to_path_buf(),
                            category: "partial sections",
                        }
                    })
                })?,
        };
        let mut symbol_bytes = BTreeMap::<String, u64>::new();
        let mut crate_bytes = BTreeMap::<String, u64>::new();
        for section in &mut self.sections {
            let included = match basis {
                AttributionBasis::Complete(sections) => sections.iter().any(|candidate| {
                    candidate.name() == section.name && candidate.load_bytes() > 0
                }),
                AttributionBasis::Partial => probably_loaded(&section.name),
            };
            if !included {
                continue;
            }
            section.validate(path)?;
            for contribution in &section.contributions {
                let Some(symbol) = demangled_symbol(&contribution.input_section) else {
                    continue;
                };
                accumulate(
                    path,
                    &mut symbol_bytes,
                    symbol.clone(),
                    contribution.range.byte_len(),
                    "symbols",
                )?;
                if let Some(crate_name) = crate_name(&symbol) {
                    accumulate(
                        path,
                        &mut crate_bytes,
                        crate_name.to_string(),
                        contribution.range.byte_len(),
                        "crates",
                    )?;
                }
            }
        }
        attribution(analyzed_bytes, crate_bytes, symbol_bytes, path)
    }
}

impl MapSection {
    fn new(name: String, range: AddressRange) -> Self {
        Self {
            name,
            range,
            contributions: Vec::new(),
        }
    }

    fn push(&mut self, contribution: MapContribution) {
        self.contributions.push(contribution);
    }

    fn validate(&mut self, path: &Path) -> Result<(), AttributionError> {
        self.contributions
            .sort_by_key(|contribution| (contribution.range.start(), contribution.range.end()));
        let mut previous: Option<&MapContribution> = None;
        for contribution in &self.contributions {
            if !self.range.contains(contribution.range) {
                return Err(AttributionError::EscapingContribution {
                    path: path.to_path_buf(),
                    output: self.name.clone(),
                    input: contribution.input_section.clone(),
                });
            }
            if previous.is_some_and(|prior| prior.range.overlaps(contribution.range)) {
                return Err(AttributionError::OverlappingContributions {
                    path: path.to_path_buf(),
                    section: self.name.clone(),
                });
            }
            previous = Some(contribution);
        }
        Ok(())
    }
}

impl MapContribution {
    fn new(input_section: String, range: AddressRange) -> Self {
        Self {
            input_section,
            range,
        }
    }
}

fn range(
    path: &Path,
    section: &str,
    start: u64,
    bytes: u64,
) -> Result<AddressRange, AttributionError> {
    AddressRange::from_start_and_size(start, bytes).map_err(|_: AddressRangeError| {
        AttributionError::InvalidRange {
            path: path.to_path_buf(),
            section: section.to_string(),
        }
    })
}

fn attribution(
    analyzed_bytes: u64,
    crate_bytes: BTreeMap<String, u64>,
    symbol_bytes: BTreeMap<String, u64>,
    path: &Path,
) -> Result<AttributionAnalysis, AttributionError> {
    let crate_coverage = coverage(analyzed_bytes, &crate_bytes, path, "crates")?;
    let symbol_coverage = coverage(analyzed_bytes, &symbol_bytes, path, "symbols")?;
    Ok(AttributionAnalysis {
        crate_coverage,
        symbol_coverage,
        largest_crates: ranked(crate_bytes),
        largest_symbols: ranked_symbols(symbol_bytes),
    })
}

fn coverage(
    analyzed_bytes: u64,
    entries: &BTreeMap<String, u64>,
    path: &Path,
    category: &'static str,
) -> Result<AttributionCoverage, AttributionError> {
    let attributed_bytes = entries.values().try_fold(0_u64, |sum, bytes| {
        sum.checked_add(*bytes)
            .ok_or_else(|| AttributionError::ArithmeticOverflow {
                path: path.to_path_buf(),
                category,
            })
    })?;
    let unclassified_bytes = analyzed_bytes
        .checked_sub(attributed_bytes)
        .ok_or_else(|| AttributionError::AttributionExceedsBasis {
            path: path.to_path_buf(),
            total: analyzed_bytes,
        })?;
    Ok(AttributionCoverage {
        analyzed_bytes,
        attributed_bytes,
        unclassified_bytes,
    })
}

fn accumulate(
    path: &Path,
    entries: &mut BTreeMap<String, u64>,
    name: String,
    bytes: u64,
    category: &'static str,
) -> Result<(), AttributionError> {
    let total = entries.entry(name).or_default();
    *total = total
        .checked_add(bytes)
        .ok_or_else(|| AttributionError::ArithmeticOverflow {
            path: path.to_path_buf(),
            category,
        })?;
    Ok(())
}

fn ranked(entries: BTreeMap<String, u64>) -> Vec<AttributedUsage> {
    let mut entries = entries
        .into_iter()
        .map(|(name, bytes)| AttributedUsage { name, bytes })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.name.cmp(&right.name))
    });
    entries.truncate(RANKED_LIMIT);
    entries
}

fn ranked_symbols(entries: BTreeMap<String, u64>) -> Vec<AttributedUsage> {
    ranked(entries)
        .into_iter()
        .map(|usage| AttributedUsage {
            name: compact_symbol(&usage.name),
            bytes: usage.bytes,
        })
        .collect()
}

fn demangled_symbol(input_section: &str) -> Option<String> {
    let start = input_section
        .find("_R")
        .or_else(|| input_section.find("_ZN"))?;
    let mut candidate = &input_section[start..];
    loop {
        if let Ok(symbol) = try_demangle(candidate) {
            return Some(format!("{symbol:#}"));
        }
        let (prefix, _) = candidate.rsplit_once('.')?;
        candidate = prefix;
    }
}

fn crate_name(symbol: &str) -> Option<&str> {
    symbol.match_indices("::").find_map(|(separator, _)| {
        let prefix = &symbol[..separator];
        let start = prefix
            .rfind(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map_or(0, |index| index + 1);
        let candidate = &prefix[start..];
        (!candidate.is_empty()).then_some(candidate)
    })
}

fn compact_symbol(symbol: &str) -> String {
    if symbol.len() <= SYMBOL_PREFIX_BYTES + SYMBOL_SUFFIX_BYTES {
        return symbol.to_string();
    }
    let prefix_end = character_boundary(symbol, SYMBOL_PREFIX_BYTES);
    let suffix_start = character_boundary(symbol, symbol.len() - SYMBOL_SUFFIX_BYTES);
    let digest = prns_flash_manifest::sha256_hex(symbol.as_bytes());
    format!(
        "{}…{} [sha256:{}]",
        &symbol[..prefix_end],
        &symbol[suffix_start..],
        &digest[..12]
    )
}

fn character_boundary(value: &str, index: usize) -> usize {
    let mut boundary = index.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn probably_loaded(section: &str) -> bool {
    [
        ".data",
        ".dram",
        ".fini",
        ".flash",
        ".gnu.sgstubs",
        ".got",
        ".init",
        ".iram",
        ".rodata",
        ".rtc_fast.data",
        ".rtc_fast.text",
        ".rtc_slow.data",
        ".rwdata",
        ".rwtext",
        ".sdata",
        ".text",
        ".trap",
        ".vector",
    ]
    .iter()
    .any(|prefix| section.starts_with(prefix))
        && !section.contains("bss")
        && !section.contains("dummy")
        && !section.contains("heap")
        && !section.contains("noinit")
        && !section.contains("stack")
        && !section.contains("uninit")
}
