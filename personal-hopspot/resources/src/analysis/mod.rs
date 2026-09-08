mod attribution;
mod elf;
pub(crate) mod executable;
mod ram;

pub(crate) use attribution::{
    analyze_linker_map, AttributionAnalysis, AttributionBasis, AttributionError,
};
pub(crate) use elf::{read_allocated_sections, AllocatedSection, AnalysisError, SectionKind};
pub(crate) use executable::{analyze as analyze_executable, ExecutableAnalysis, ExecutableError};
pub(crate) use ram::{analyze as analyze_ram, RamAnalysisError, RamCapacity};
