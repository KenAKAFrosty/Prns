mod elf;
mod ram;

pub(crate) use elf::{read_allocated_sections, AllocatedSection, AnalysisError, SectionKind};
pub(crate) use ram::{analyze as analyze_ram, RamAnalysisError, RamCapacity};
