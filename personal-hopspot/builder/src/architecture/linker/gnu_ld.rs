use super::MemoryOverflows;

pub(in crate::architecture) fn detect(diagnostics: &str) -> Option<MemoryOverflows> {
    MemoryOverflows::from_entries(diagnostics.lines().filter_map(parse_line))
}

fn parse_line(line: &str) -> Option<(&str, u64)> {
    let (_, overflow) = line.split_once("region `")?;
    let (region, amount) = overflow.split_once("' overflowed by ")?;
    let bytes = amount.split_whitespace().next()?.parse().ok()?;
    Some((region, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_gnu_regions_remain_distinct() {
        let diagnostics = "ld: region `iram0_0_seg' overflowed by 32 bytes\nld: region `dram0_0_seg' overflowed by 64 bytes";
        let overflows = detect(diagnostics).expect("memory overflow");
        let regions = overflows
            .iter()
            .map(|overflow| (overflow.region(), overflow.bytes()))
            .collect::<Vec<_>>();
        assert_eq!(regions, vec![("dram0_0_seg", 64), ("iram0_0_seg", 32)]);
        assert_eq!(overflows.primary().region(), "dram0_0_seg");
    }

    #[test]
    fn malformed_overflow_diagnostics_are_not_accepted() {
        assert!(detect("ld: region `dram0_0_seg' overflowed").is_none());
    }
}
