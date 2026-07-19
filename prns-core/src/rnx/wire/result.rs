use rmp::encode;

use super::core::validate_result_ref;
use super::RnxCodecError;
use crate::rnx::{ExecutionConclusion, ExecutionResultRef};

pub trait RnxEncodeSink {
    type Error;

    fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeExecutionResultError<E> {
    Codec(RnxCodecError),
    Sink(E),
}

pub fn encode_execution_result_to(
    result: ExecutionResultRef<'_>,
    output: &mut [u8],
) -> Result<usize, RnxCodecError> {
    let capacity = output.len();
    let mut sink = SliceSink { remaining: output };
    encode_execution_result_into(result, &mut sink).map_err(|error| match error {
        EncodeExecutionResultError::Codec(error) => error,
        EncodeExecutionResultError::Sink(()) => RnxCodecError::BufferTooShort,
    })?;
    Ok(capacity - sink.remaining.len())
}

pub fn encode_execution_result_into<S: RnxEncodeSink>(
    result: ExecutionResultRef<'_>,
    sink: &mut S,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    validate_result_ref(result).map_err(EncodeExecutionResultError::Codec)?;
    write_array_len(sink, 8)?;
    match result {
        ExecutionResultRef::NotExecuted { started_at } => {
            write_bool(sink, false)?;
            for _ in 0..5 {
                write_nil(sink)?;
            }
            write_f64(sink, started_at)?;
            write_nil(sink)?;
        }
        ExecutionResultRef::Executed(executed) => {
            write_bool(sink, true)?;
            match executed.return_code {
                Some(code) => write_i64(sink, i64::from(code))?,
                None => write_nil(sink)?,
            }
            write_binary(sink, executed.stdout)?;
            write_binary(sink, executed.stderr)?;
            write_u64(sink, executed.total_stdout)?;
            write_u64(sink, executed.total_stderr)?;
            write_f64(sink, executed.started_at)?;
            match executed.conclusion {
                ExecutionConclusion::CompletedAt(at) => write_f64(sink, at)?,
                ExecutionConclusion::TimedOut => write_nil(sink)?,
            }
        }
    }
    Ok(())
}

struct SliceSink<'a> {
    remaining: &'a mut [u8],
}

impl RnxEncodeSink for SliceSink<'_> {
    type Error = ();

    fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes.len() > self.remaining.len() {
            return Err(());
        }
        let remaining = core::mem::take(&mut self.remaining);
        let (written, tail) = remaining.split_at_mut(bytes.len());
        written.copy_from_slice(bytes);
        self.remaining = tail;
        Ok(())
    }
}

fn write_array_len<S: RnxEncodeSink>(
    sink: &mut S,
    length: u32,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_array_len(output, length)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_bool<S: RnxEncodeSink>(
    sink: &mut S,
    value: bool,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_bool(output, value).map_err(|_| ())
    })
}

fn write_nil<S: RnxEncodeSink>(sink: &mut S) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| encode::write_nil(output).map_err(|_| ()))
}

fn write_i64<S: RnxEncodeSink>(
    sink: &mut S,
    value: i64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_sint(output, value)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_u64<S: RnxEncodeSink>(
    sink: &mut S,
    value: u64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_uint(output, value)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_f64<S: RnxEncodeSink>(
    sink: &mut S,
    value: f64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_f64(output, value).map(|_| ()).map_err(|_| ())
    })
}

fn write_binary<S: RnxEncodeSink>(
    sink: &mut S,
    value: &[u8],
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    let length = u32::try_from(value.len())
        .map_err(|_| EncodeExecutionResultError::Codec(RnxCodecError::IncoherentResult))?;
    write_header(sink, |output| {
        encode::write_bin_len(output, length)
            .map(|_| ())
            .map_err(|_| ())
    })?;
    sink.put(value).map_err(EncodeExecutionResultError::Sink)
}

fn write_header<S: RnxEncodeSink>(
    sink: &mut S,
    encode: impl FnOnce(&mut &mut [u8]) -> Result<(), ()>,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    let mut header = [0u8; 9];
    let capacity = header.len();
    let mut remaining = header.as_mut_slice();
    encode(&mut remaining)
        .map_err(|()| EncodeExecutionResultError::Codec(RnxCodecError::IncoherentResult))?;
    let written = capacity - remaining.len();
    sink.put(&header[..written])
        .map_err(EncodeExecutionResultError::Sink)
}
