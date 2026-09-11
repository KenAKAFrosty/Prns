mod aggregate;
mod discovery;
mod record;
mod validation;

pub use aggregate::{assemble, AggregateError};
pub use discovery::{discover_proofs, discover_resources, DiscoveryError};
pub(crate) use record::{
    record_miri, record_platform_emulation, record_target_isa, MiriRecordRequest,
    PlatformRecordRequest, RecordError, TargetIsaRecordRequest,
};
pub(crate) use validation::validate_documents_current;
pub use validation::{
    load_canonical_matrix, load_matrix, validate_canonical as validate_matrix, validate_current,
    MatrixValidationError,
};
