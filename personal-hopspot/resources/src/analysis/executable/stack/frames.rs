use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use gimli::{
    BaseAddresses, CfaRule, CieOrFde, DebugFrame, LittleEndian, UnwindContext, UnwindSection,
};
use object::{Object, ObjectSection, SectionFlags};
use personal_hopspot_builder::architecture::StackFrameEvidence;

use super::{StackFrame, StackMetadataError};
use crate::analysis::executable::{FunctionAnalysis, LoadSegment};

const STACK_SIZE_SECTION: &str = ".stack_sizes";

pub(super) struct FrameAnalysis {
    pub(super) section_bytes: u64,
    pub(super) frames: Vec<StackFrame>,
    pub(super) missing_functions: u64,
    pub(super) foreign_or_assembly_functions: u64,
    pub(super) unmatched_records: u64,
    pub(super) unsupported_records: u64,
}

pub(super) fn analyze(
    path: &Path,
    object: &object::File<'_>,
    functions: &FunctionAnalysis,
    load_segments: &[LoadSegment],
    normalize: fn(u64) -> u64,
    source: StackFrameEvidence,
    dwarf_cfa_registers: &[u16],
) -> Result<FrameAnalysis, StackMetadataError> {
    let section_name = match source {
        StackFrameEvidence::LlvmStackSizes => STACK_SIZE_SECTION,
        StackFrameEvidence::DwarfDebugFrame => ".debug_frame",
    };
    let section =
        object
            .section_by_name(section_name)
            .ok_or_else(|| StackMetadataError::MissingSection {
                path: path.to_path_buf(),
                section: section_name,
            })?;
    if matches!(section.flags(), SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0)
    {
        return Err(StackMetadataError::AllocatedSection {
            path: path.to_path_buf(),
            section: section_name,
        });
    }
    if let Some((offset, bytes)) = section.file_range() {
        let end = offset
            .checked_add(bytes)
            .ok_or(StackMetadataError::SectionRangeOverflow { offset, bytes })?;
        if load_segments.iter().any(|segment| {
            let segment_end = segment.file_offset.saturating_add(segment.file_bytes);
            segment.file_offset < end && offset < segment_end
        }) {
            return Err(StackMetadataError::LoadableSection {
                path: path.to_path_buf(),
                section: section_name,
            });
        }
    }
    let data = section
        .data()
        .map_err(|source| StackMetadataError::ReadSection {
            path: path.to_path_buf(),
            section: section_name,
            source,
        })?;
    let (records, unsupported_records) = match source {
        StackFrameEvidence::LlvmStackSizes => (parse_stack_sizes(data, normalize)?, 0),
        StackFrameEvidence::DwarfDebugFrame => {
            parse_debug_frame(data, normalize, dwarf_cfa_registers)?
        }
    };
    let boundaries = functions
        .boundaries
        .iter()
        .map(|boundary| (normalize(boundary.range.start()), boundary))
        .collect::<BTreeMap<_, _>>();
    let mut frames = Vec::with_capacity(records.len());
    let mut unmatched_records = 0_u64;
    for (address, bytes) in &records {
        let name = boundaries.get(address).map_or_else(
            || {
                unmatched_records += 1;
                format!("<unclassified@{address:#x}>")
            },
            |boundary| boundary.name.clone(),
        );
        frames.push(StackFrame {
            name,
            address: *address,
            bytes: *bytes,
        });
    }
    frames.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.address.cmp(&right.address))
    });

    let framed = records.keys().copied().collect::<BTreeSet<_>>();
    let mut function_starts = BTreeMap::new();
    for boundary in &functions.boundaries {
        function_starts
            .entry(normalize(boundary.range.start()))
            .or_insert(&boundary.name);
    }
    let missing = function_starts
        .iter()
        .filter(|(address, _)| !framed.contains(address))
        .collect::<Vec<_>>();
    let foreign_or_assembly_functions = missing
        .iter()
        .filter(|(_, name)| !name.contains("::"))
        .count()
        .try_into()
        .map_err(|_| StackMetadataError::CountOverflow {
            evidence: "foreign or assembly functions",
        })?;

    Ok(FrameAnalysis {
        section_bytes: data
            .len()
            .try_into()
            .map_err(|_| StackMetadataError::CountOverflow {
                evidence: "stack metadata section bytes",
            })?,
        frames,
        missing_functions: missing.len().try_into().map_err(|_| {
            StackMetadataError::CountOverflow {
                evidence: "functions without stack metadata",
            }
        })?,
        foreign_or_assembly_functions,
        unmatched_records,
        unsupported_records,
    })
}

fn parse_stack_sizes(
    data: &[u8],
    normalize: fn(u64) -> u64,
) -> Result<BTreeMap<u64, u64>, StackMetadataError> {
    let mut offset = 0_usize;
    let mut records = BTreeMap::<u64, u64>::new();
    while offset < data.len() {
        let address_end = offset
            .checked_add(4)
            .ok_or(StackMetadataError::MalformedAddress { offset })?;
        let address = data
            .get(offset..address_end)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_le_bytes)
            .map(u64::from)
            .ok_or(StackMetadataError::MalformedAddress { offset })?;
        offset = address_end;
        let (bytes, consumed) = decode_uleb128(&data[offset..], offset)?;
        offset += consumed;
        let address = normalize(address);
        if let Some(previous) = records.insert(address, bytes) {
            if previous != bytes {
                return Err(StackMetadataError::ConflictingFrame {
                    address,
                    first: previous,
                    second: bytes,
                });
            }
        }
    }
    if records.is_empty() {
        return Err(StackMetadataError::EmptySection {
            section: STACK_SIZE_SECTION,
        });
    }
    Ok(records)
}

fn parse_debug_frame(
    data: &[u8],
    normalize: fn(u64) -> u64,
    cfa_registers: &[u16],
) -> Result<(BTreeMap<u64, u64>, u64), StackMetadataError> {
    if data.is_empty() {
        return Err(StackMetadataError::EmptySection {
            section: ".debug_frame",
        });
    }
    let mut section = DebugFrame::new(data, LittleEndian);
    section.set_address_size(4);
    let bases = BaseAddresses::default();
    let mut entries = section.entries(&bases);
    let mut records = BTreeMap::<u64, u64>::new();
    let mut unsupported_records = 0_u64;
    while let Some(entry) = entries.next()? {
        let CieOrFde::Fde(partial) = entry else {
            continue;
        };
        let fde = partial.parse(DebugFrame::cie_from_offset)?;
        let mut context = UnwindContext::new();
        let mut rows = fde.rows(&section, &bases, &mut context)?;
        let mut frame_bytes = None;
        let mut unsupported = false;
        while let Some(row) = rows.next_row()? {
            match row.cfa() {
                CfaRule::RegisterAndOffset { register, offset }
                    if cfa_registers.contains(&register.0) && *offset >= 0 =>
                {
                    let bytes =
                        u64::try_from(*offset).map_err(|_| StackMetadataError::CountOverflow {
                            evidence: "DWARF CFA offset",
                        })?;
                    frame_bytes =
                        Some(frame_bytes.map_or(bytes, |current: u64| current.max(bytes)));
                }
                _ => unsupported = true,
            }
        }
        if unsupported {
            unsupported_records =
                unsupported_records
                    .checked_add(1)
                    .ok_or(StackMetadataError::CountOverflow {
                        evidence: "unsupported DWARF frame records",
                    })?;
        }
        let Some(frame_bytes) = frame_bytes else {
            continue;
        };
        let address = normalize(fde.initial_address());
        records
            .entry(address)
            .and_modify(|current| *current = (*current).max(frame_bytes))
            .or_insert(frame_bytes);
    }
    if records.is_empty() {
        return Err(StackMetadataError::NoSupportedDwarfFrames);
    }
    Ok((records, unsupported_records))
}

fn decode_uleb128(data: &[u8], offset: usize) -> Result<(u64, usize), StackMetadataError> {
    let mut value = 0_u64;
    for (index, byte) in data.iter().copied().enumerate().take(10) {
        let shift =
            u32::try_from(index * 7).map_err(|_| StackMetadataError::MalformedSize { offset })?;
        let payload = u64::from(byte & 0x7f);
        if shift == 63 && payload > 1 {
            return Err(StackMetadataError::MalformedSize { offset });
        }
        let payload = payload
            .checked_shl(shift)
            .ok_or(StackMetadataError::MalformedSize { offset })?;
        value = value
            .checked_add(payload)
            .ok_or(StackMetadataError::MalformedSize { offset })?;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(StackMetadataError::MalformedSize { offset })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_elf_stack_records_use_addresses_and_uleb128_sizes() {
        let records = parse_stack_sizes(
            &[
                0x00, 0x10, 0x00, 0x00, 0x20, 0x04, 0x10, 0x00, 0x00, 0xac, 0x02,
            ],
            |address| address,
        )
        .expect("stack records");

        assert_eq!(records, BTreeMap::from([(0x1000, 32), (0x1004, 300)]));
    }

    #[test]
    fn truncated_and_conflicting_records_are_rejected() {
        assert_eq!(
            parse_stack_sizes(&[0, 1, 2], |address| address),
            Err(StackMetadataError::MalformedAddress { offset: 0 })
        );
        assert_eq!(
            parse_stack_sizes(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 2], |address| address),
            Err(StackMetadataError::ConflictingFrame {
                address: 0,
                first: 1,
                second: 2,
            })
        );
        let mut oversized = vec![0, 0, 0, 0];
        oversized.extend([0x80; 9]);
        oversized.push(0x02);
        assert_eq!(
            parse_stack_sizes(&oversized, |address| address),
            Err(StackMetadataError::MalformedSize { offset: 4 })
        );
    }

    #[test]
    fn dwarf_rows_report_the_largest_stack_pointer_offset() {
        let data = [
            0x10, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x04, 0x00, 0x04, 0x00, 0x01, 0x7c,
            0x01, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x30, 0x04, 0x80, 0x40, 0x26, 0x01, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x7a, 0x00, 0x04, 0x42, 0xac, 0x00, 0x00, 0x00, 0x07, 0x01, 0x00, 0x00,
            0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x56, 0x01, 0x04, 0x42, 0x0e, 0x00,
            0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x01, 0x04, 0x42,
            0x1e, 0x00, 0x00, 0x00, 0x42, 0x0e, 0x20, 0x44, 0x81, 0x01, 0x88, 0x02,
        ];

        let (records, unsupported) =
            parse_debug_frame(&data, |address| address, &[2]).expect("DWARF frames");

        assert_eq!(
            records,
            BTreeMap::from([
                (0x4080_0430, 0),
                (0x4204_007a, 0),
                (0x4204_0156, 0),
                (0x4204_0164, 32),
            ])
        );
        assert_eq!(unsupported, 0);
    }
}
