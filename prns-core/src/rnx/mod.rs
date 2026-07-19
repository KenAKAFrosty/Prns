mod codec;
mod model;

#[cfg(feature = "alloc")]
pub use codec::{
    decode_execution_request, decode_execution_result, encode_execution_request,
    encode_execution_result,
};
pub use codec::{
    decode_execution_request_ref, encode_execution_result_into, encode_execution_result_to,
    EncodeExecutionResultError, RnxCodecError, RnxEncodeSink, RnxField,
};
#[cfg(feature = "alloc")]
pub use model::{ExecutedCommand, ExecutionRequest, ExecutionResult};
pub use model::{ExecutedCommandRef, ExecutionConclusion, ExecutionRequestRef, ExecutionResultRef};

pub const APP_NAME: &str = "rnx";
pub const EXECUTE_ASPECT: &str = "execute";
pub const COMMAND_PATH: &str = "command";
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_EXECUTION_REQUEST_BYTES: usize = crate::routing::links::resources::MAX_EFFICIENT_SIZE;
pub const MAX_STDIN_BYTES: usize = MAX_EXECUTION_REQUEST_BYTES;
pub const MAX_RETURNED_STREAM_BYTES: usize = 16 * 1024 * 1024;
