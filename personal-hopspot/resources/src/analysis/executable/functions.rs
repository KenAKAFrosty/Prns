use object::{Object, ObjectSymbol};
use personal_hopspot_memory::AddressRange;
use rustc_demangle::try_demangle;

use super::{coverage, ExecutableError, ExecutableSection};

const LARGEST_FUNCTION_LIMIT: usize = 20;
const SYMBOL_PREFIX_BYTES: usize = 160;
const SYMBOL_SUFFIX_BYTES: usize = 80;

#[derive(Debug)]
pub(crate) struct FunctionAnalysis {
    pub(crate) classified_bytes: u64,
    pub(crate) unclassified_bytes: u64,
    pub(crate) boundaries: Vec<FunctionBoundary>,
    pub(crate) largest: Vec<FunctionBoundary>,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionBoundary {
    pub(crate) name: String,
    pub(crate) range: AddressRange,
    pub(crate) fingerprint: String,
}

pub(super) fn analyze(
    object: &object::File<'_>,
    sections: &[ExecutableSection],
    normalize: fn(u64) -> u64,
) -> Result<FunctionAnalysis, ExecutableError> {
    let mut boundaries = object
        .symbols()
        .filter(|symbol| {
            symbol.is_definition()
                && symbol.kind() == object::SymbolKind::Text
                && symbol.size() != 0
        })
        .filter_map(|symbol| {
            let address = normalize(symbol.address());
            let bytes = symbol.size();
            let range = AddressRange::from_start_and_size(address, bytes).ok()?;
            let section = sections
                .iter()
                .find(|section| section.range.contains(range))?;
            let start = usize::try_from(address.checked_sub(section.range.start())?).ok()?;
            let end = usize::try_from(range.end().checked_sub(section.range.start())?).ok()?;
            let name = symbol.name().ok().map(compact_symbol)?;
            Some(FunctionBoundary {
                name,
                range,
                fingerprint: prns_flash_manifest::sha256_hex(&section.data[start..end]),
            })
        })
        .collect::<Vec<_>>();
    boundaries.sort_by(|left, right| {
        left.range
            .start()
            .cmp(&right.range.start())
            .then_with(|| left.range.end().cmp(&right.range.end()))
            .then_with(|| left.name.cmp(&right.name))
    });
    boundaries.dedup_by(|left, right| {
        left.range == right.range
            && left.name == right.name
            && left.fingerprint == right.fingerprint
    });

    let classified_bytes = coverage::covered_bytes(
        boundaries.iter().map(|boundary| boundary.range).collect(),
        "classified function bytes",
    )?;
    let executable_bytes = sections.iter().try_fold(0_u64, |total, section| {
        total.checked_add(section.range.byte_len())
    });
    let executable_bytes = executable_bytes.ok_or(ExecutableError::InvalidRange {
        kind: "executable section total",
        start: 0,
        bytes: u64::MAX,
    })?;
    let unclassified_bytes = executable_bytes.checked_sub(classified_bytes).ok_or(
        ExecutableError::InvalidFunctionCoverage {
            classified: classified_bytes,
            executable: executable_bytes,
        },
    )?;
    let mut largest = boundaries.clone();
    largest.sort_by(|left, right| {
        right
            .range
            .byte_len()
            .cmp(&left.range.byte_len())
            .then_with(|| left.name.cmp(&right.name))
    });
    largest.truncate(LARGEST_FUNCTION_LIMIT);

    Ok(FunctionAnalysis {
        classified_bytes,
        unclassified_bytes,
        boundaries,
        largest,
    })
}

fn compact_symbol(symbol: &str) -> String {
    let demangled = try_demangle(symbol)
        .map(|value| format!("{value:#}"))
        .unwrap_or_else(|_| symbol.to_string());
    if demangled.len() <= SYMBOL_PREFIX_BYTES + SYMBOL_SUFFIX_BYTES {
        return demangled;
    }
    let prefix_end = character_boundary(&demangled, SYMBOL_PREFIX_BYTES);
    let suffix_start = character_boundary(&demangled, demangled.len() - SYMBOL_SUFFIX_BYTES);
    let digest = prns_flash_manifest::sha256_hex(demangled.as_bytes());
    format!(
        "{}…{} [sha256:{}]",
        &demangled[..prefix_end],
        &demangled[suffix_start..],
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
