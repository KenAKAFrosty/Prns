use super::MemoryOverflows;

pub(in crate::architecture) fn detect(diagnostics: &str) -> Option<MemoryOverflows> {
    MemoryOverflows::from_entries(diagnostics.lines().filter_map(parse_line))
}

fn parse_line(line: &str) -> Option<(&str, u64)> {
    let (_, overflow) = line.split_once(" will not fit in region '")?;
    let (region, amount) = overflow.split_once("': overflowed by ")?;
    let bytes = amount.split_whitespace().next()?.parse().ok()?;
    Some((region, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progressive_flash_overflows_collapse_to_the_final_extent() {
        let diagnostics = "rust-lld: error: section '.rodata' will not fit in region 'FLASH': overflowed by 66304 bytes\nrust-lld: error: section '.data' will not fit in region 'FLASH': overflowed by 86228 bytes\nrust-lld: error: section '.gnu.sgstubs' will not fit in region 'FLASH': overflowed by 86240 bytes";
        let overflows = detect(diagnostics).expect("memory overflow");
        let regions = overflows.iter().collect::<Vec<_>>();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].region(), "FLASH");
        assert_eq!(regions[0].bytes(), 86_240);
    }

    #[test]
    fn unrelated_link_failures_are_not_memory_overflows() {
        assert!(detect("rust-lld: error: undefined symbol: missing").is_none());
    }
}
