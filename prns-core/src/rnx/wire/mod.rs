mod core;
mod impls;
mod request;
mod result;
#[cfg(test)]
mod tests;

pub use impls::*;
pub use request::decode_execution_request_ref;
pub use result::{
    encode_execution_result_into, encode_execution_result_to, EncodeExecutionResultError,
    RnxEncodeSink,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnxField {
    Command,
    Timeout,
    StdoutLimit,
    StderrLimit,
    Stdin,
    Executed,
    ReturnCode,
    Stdout,
    Stderr,
    TotalStdout,
    TotalStderr,
    Started,
    Concluded,
}
