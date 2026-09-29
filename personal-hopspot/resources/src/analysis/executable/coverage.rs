use personal_hopspot_memory::AddressRange;

use super::ExecutableError;

pub(super) fn covered_bytes(
    mut ranges: Vec<AddressRange>,
    evidence: &'static str,
) -> Result<u64, ExecutableError> {
    ranges.sort_by_key(|range| (range.start(), range.end()));
    let mut total = 0_u64;
    let mut current: Option<AddressRange> = None;
    for range in ranges {
        current = match current {
            None => Some(range),
            Some(active) if range.start() <= active.end() => Some(AddressRange::new(
                active.start(),
                active.end().max(range.end()),
            )),
            Some(active) => {
                total = total
                    .checked_add(active.byte_len())
                    .ok_or(ExecutableError::CountOverflow { evidence })?;
                Some(range)
            }
        };
    }
    current.map_or(Ok(total), |range| {
        total
            .checked_add(range.byte_len())
            .ok_or(ExecutableError::CountOverflow { evidence })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_ranges_are_counted_once() {
        let ranges = vec![
            AddressRange::new(4, 12),
            AddressRange::new(0, 8),
            AddressRange::new(20, 24),
        ];
        assert_eq!(covered_bytes(ranges, "fixture").expect("valid ranges"), 16);
    }
}
