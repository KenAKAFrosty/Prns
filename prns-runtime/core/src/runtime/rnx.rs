use prns_core::identity::IdentityHash;
use prns_core::rnx::{
    decode_execution_request_ref, encode_execution_result_into, EncodeExecutionResultError,
    ExecutedCommandRef, ExecutionConclusion, ExecutionRequestRef, ExecutionResultRef,
    RnxEncodeSink, MAX_RETURNED_STREAM_BYTES,
};
use prns_core::wire::DestinationHash;

use super::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, ResponseCapacityExceeded,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnxAuthorization {
    DenyAll,
    AllowList(&'static [IdentityHash]),
    Public,
}

impl RnxAuthorization {
    const fn endpoint_policy(self) -> RequestEndpointPolicy {
        match self {
            Self::DenyAll => RequestEndpointPolicy::AllowNone,
            Self::AllowList(identities) => RequestEndpointPolicy::AllowList(identities),
            Self::Public => RequestEndpointPolicy::AllowAll,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RnxCompletion {
    NotExecuted {
        started_at: f64,
    },
    Executed {
        return_code: Option<i32>,
        started_at: f64,
        conclusion: ExecutionConclusion,
    },
}

pub trait RnxOutputBuffer {
    fn put(&mut self, bytes: &[u8]) -> usize;
    fn as_slice(&self) -> &[u8];
}

impl<const N: usize> RnxOutputBuffer for heapless::Vec<u8, N> {
    fn put(&mut self, bytes: &[u8]) -> usize {
        let accepted = bytes.len().min(self.capacity().saturating_sub(self.len()));
        let _ = self.extend_from_slice(&bytes[..accepted]);
        accepted
    }

    fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

impl RnxOutputBuffer for alloc::vec::Vec<u8> {
    fn put(&mut self, bytes: &[u8]) -> usize {
        self.extend_from_slice(bytes);
        bytes.len()
    }

    fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

pub trait RnxOutputStorage: Default {
    fn buffers(&mut self) -> (&mut dyn RnxOutputBuffer, &mut dyn RnxOutputBuffer);
}

pub struct FixedRnxOutput<const STDOUT: usize, const STDERR: usize> {
    stdout: heapless::Vec<u8, STDOUT>,
    stderr: heapless::Vec<u8, STDERR>,
}

impl<const STDOUT: usize, const STDERR: usize> Default for FixedRnxOutput<STDOUT, STDERR> {
    fn default() -> Self {
        Self {
            stdout: heapless::Vec::new(),
            stderr: heapless::Vec::new(),
        }
    }
}

impl<const STDOUT: usize, const STDERR: usize> RnxOutputStorage for FixedRnxOutput<STDOUT, STDERR> {
    fn buffers(&mut self) -> (&mut dyn RnxOutputBuffer, &mut dyn RnxOutputBuffer) {
        (&mut self.stdout, &mut self.stderr)
    }
}

#[derive(Default)]
pub struct HeapRnxOutput {
    stdout: alloc::vec::Vec<u8>,
    stderr: alloc::vec::Vec<u8>,
}

impl RnxOutputStorage for HeapRnxOutput {
    fn buffers(&mut self) -> (&mut dyn RnxOutputBuffer, &mut dyn RnxOutputBuffer) {
        (&mut self.stdout, &mut self.stderr)
    }
}

struct CapturedOutput<'a> {
    buffer: &'a mut dyn RnxOutputBuffer,
    returned_limit: Option<u64>,
    total: u64,
}

impl CapturedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let remaining = self.returned_limit.map_or(u64::MAX, |limit| {
            limit.saturating_sub(self.buffer.as_slice().len() as u64)
        });
        let accepted = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        self.buffer.put(&bytes[..accepted]);
    }

    fn observe_total(&mut self, total: u64) {
        self.total = self.total.max(total);
    }
}

pub struct RnxOutput<'a> {
    stdout: CapturedOutput<'a>,
    stderr: CapturedOutput<'a>,
}

impl<'a> RnxOutput<'a> {
    pub fn new<T: RnxOutputStorage>(
        storage: &'a mut T,
        stdout_limit: Option<u64>,
        stderr_limit: Option<u64>,
    ) -> Self {
        let (stdout, stderr) = storage.buffers();
        let stdout_limit = Some(
            stdout_limit
                .unwrap_or(MAX_RETURNED_STREAM_BYTES as u64)
                .min(MAX_RETURNED_STREAM_BYTES as u64),
        );
        let stderr_limit = Some(
            stderr_limit
                .unwrap_or(MAX_RETURNED_STREAM_BYTES as u64)
                .min(MAX_RETURNED_STREAM_BYTES as u64),
        );
        Self {
            stdout: CapturedOutput {
                buffer: stdout,
                returned_limit: stdout_limit,
                total: 0,
            },
            stderr: CapturedOutput {
                buffer: stderr,
                returned_limit: stderr_limit,
                total: 0,
            },
        }
    }

    pub fn stdout(&mut self, bytes: &[u8]) {
        self.stdout.write(bytes);
    }

    pub fn stderr(&mut self, bytes: &[u8]) {
        self.stderr.write(bytes);
    }

    pub fn observe_total_stdout(&mut self, total: u64) {
        self.stdout.observe_total(total);
    }

    pub fn observe_total_stderr(&mut self, total: u64) {
        self.stderr.observe_total(total);
    }

    #[must_use]
    pub fn stdout_bytes(&self) -> &[u8] {
        self.stdout.buffer.as_slice()
    }

    #[must_use]
    pub fn stderr_bytes(&self) -> &[u8] {
        self.stderr.buffer.as_slice()
    }

    #[must_use]
    pub fn total_stdout(&self) -> u64 {
        self.stdout.total
    }

    #[must_use]
    pub fn total_stderr(&self) -> u64 {
        self.stderr.total
    }

    fn result(&self, completion: RnxCompletion) -> ExecutionResultRef<'_> {
        match completion {
            RnxCompletion::NotExecuted { started_at } => {
                ExecutionResultRef::NotExecuted { started_at }
            }
            RnxCompletion::Executed {
                return_code,
                started_at,
                conclusion,
            } => ExecutionResultRef::Executed(ExecutedCommandRef {
                return_code,
                stdout: self.stdout.buffer.as_slice(),
                stderr: self.stderr.buffer.as_slice(),
                total_stdout: self.stdout.total,
                total_stderr: self.stderr.total,
                started_at,
                conclusion,
            }),
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait RnxCommandHandler<State> {
    const AUTHORIZATION: RnxAuthorization = RnxAuthorization::DenyAll;
    type Output: RnxOutputStorage;

    fn destination(state: &State) -> DestinationHash;

    async fn execute(
        state: &State,
        request: ExecutionRequestRef<'_>,
        output: &mut RnxOutput<'_>,
    ) -> RnxCompletion;
}

pub struct RnxRequestEndpoint<Handler>(core::marker::PhantomData<Handler>);

impl<State, Handler> RequestEndpoint<State> for RnxRequestEndpoint<Handler>
where
    Handler: RnxCommandHandler<State>,
{
    const ENDPOINT_ID: &'static str = prns_core::rnx::COMMAND_PATH;
    const POLICY: RequestEndpointPolicy = Handler::AUTHORIZATION.endpoint_policy();

    async fn handle(
        mut context: RequestContext<'_, State>,
        _node: &impl super::PrnsNodeApi,
    ) -> Result<(), Decline> {
        if context.destination != Handler::destination(context.state) {
            return Err(Decline::Ignore);
        }
        let request = decode_execution_request_ref(context.data).map_err(|_| Decline::Ignore)?;
        let mut storage = Handler::Output::default();
        let mut output = RnxOutput::new(&mut storage, request.stdout_limit, request.stderr_limit);
        let completion = Handler::execute(context.state, request, &mut output).await;
        let result = output.result(completion);
        encode_execution_result_into(result, &mut ContextSink(&mut context)).map_err(|error| {
            match error {
                EncodeExecutionResultError::Codec(_) => Decline::Ignore,
                EncodeExecutionResultError::Sink(_) => Decline::ResponseTooLarge,
            }
        })
    }
}

struct ContextSink<'a, 'request, State>(&'a mut RequestContext<'request, State>);

impl<State> RnxEncodeSink for ContextSink<'_, '_, State> {
    type Error = ResponseCapacityExceeded;

    fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write_packed(bytes).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::engine::InstantMillis;
    use prns_core::identity::IdentityHash;
    use prns_core::rnx::{
        decode_execution_result, encode_execution_request, ExecutionRequest, ExecutionResult,
    };
    use prns_core::routing::links::request::RequestId;
    use prns_core::routing::links::LinkId;
    use prns_core::routing::request_handlers::RequestPathHash;
    use prns_core::units::RttMillis;

    const DESTINATION: DestinationHash = DestinationHash::new([0x44; 16]);
    const ADMIN: IdentityHash = IdentityHash::new([0x55; 16]);

    struct App;
    struct DeniedCommand;
    struct RnxCommand;

    impl RnxCommandHandler<App> for DeniedCommand {
        type Output = FixedRnxOutput<0, 0>;

        fn destination(_state: &App) -> DestinationHash {
            DESTINATION
        }

        async fn execute(
            _state: &App,
            _request: ExecutionRequestRef<'_>,
            _output: &mut RnxOutput<'_>,
        ) -> RnxCompletion {
            RnxCompletion::NotExecuted { started_at: 1.0 }
        }
    }

    impl RnxCommandHandler<App> for RnxCommand {
        const AUTHORIZATION: RnxAuthorization = RnxAuthorization::AllowList(&[ADMIN]);
        type Output = FixedRnxOutput<4, 2>;

        fn destination(_state: &App) -> DestinationHash {
            DESTINATION
        }

        async fn execute(
            _state: &App,
            request: ExecutionRequestRef<'_>,
            output: &mut RnxOutput<'_>,
        ) -> RnxCompletion {
            if request.command != "status" {
                return RnxCompletion::NotExecuted { started_at: 1.0 };
            }
            output.stdout(b"ready");
            output.stderr(b"warn");
            RnxCompletion::Executed {
                return_code: Some(0),
                started_at: 1.0,
                conclusion: ExecutionConclusion::CompletedAt(2.0),
            }
        }
    }

    #[test]
    fn the_endpoint_adapter_bounds_its_output() {
        futures_executor::block_on(async {
            async fn dispatch<R: super::super::request_endpoints::RequestEndpointSet<App>>(
                _endpoints: &R,
                destination: DestinationHash,
                sink: &mut dyn super::super::request_endpoints::ResponseSink,
            ) -> Result<(), Decline> {
                let request = ExecutionRequest {
                    command: alloc::string::String::from("status"),
                    timeout_seconds: None,
                    stdout_limit: None,
                    stderr_limit: None,
                    stdin: None,
                };
                let data = encode_execution_request(&request).unwrap();
                let inbound = super::super::request_endpoints::InboundRequest::new(
                    destination,
                    LinkId::new([1; 16]),
                    RequestId([2; 16]),
                    Some(ADMIN),
                    InstantMillis(3),
                    RttMillis::new(4),
                    &data,
                );
                super::super::request_endpoints::dispatch_request::<App, R>(
                    &App,
                    &(),
                    RequestPathHash::of(prns_core::rnx::COMMAND_PATH),
                    inbound,
                    sink,
                )
                .await
            }

            type DeniedEndpoint = RnxRequestEndpoint<DeniedCommand>;
            type CommandEndpoint = RnxRequestEndpoint<RnxCommand>;
            let endpoints = crate::request_endpoints![CommandEndpoint];
            assert_eq!(
                <DeniedEndpoint as RequestEndpoint<App>>::POLICY,
                RequestEndpointPolicy::AllowNone,
            );
            assert_eq!(
                <CommandEndpoint as RequestEndpoint<App>>::POLICY,
                RequestEndpointPolicy::AllowList(&[ADMIN])
            );
            let mut encoded = heapless::Vec::<u8, 128>::new();
            assert_eq!(
                dispatch(&endpoints, DESTINATION, &mut encoded).await,
                Ok(())
            );
            let ExecutionResult::Executed(result) =
                decode_execution_result(encoded.as_slice()).unwrap()
            else {
                panic!("executed result");
            };
            assert_eq!(result.stdout, b"read");
            assert_eq!(result.stderr, b"wa");
            assert_eq!(result.total_stdout, 5);
            assert_eq!(result.total_stderr, 4);

            let mut wrong_destination = heapless::Vec::<u8, 128>::new();
            assert_eq!(
                dispatch(
                    &endpoints,
                    DestinationHash::new([0x66; 16]),
                    &mut wrong_destination,
                )
                .await,
                Err(Decline::Ignore)
            );
            assert!(wrong_destination.is_empty());
        });
    }
}
