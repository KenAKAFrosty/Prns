use crate::remote_control::adapter::{Runtime, PAIRS};
use personal_rns::runtime::CryptoPoolConfig;
use prns_runtime_tokio::runtime::ControlledCrypto;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;

const CRYPTO_TRACE_CAPACITY: usize = 65536;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    Inline,
    ControlledOne,
    ControlledFour,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub runtimes: [Runtime; 2],
    pub execution: Execution,
}
impl Profile {
    pub fn crypto(&self, runtime: &Runtime) -> CryptoPoolConfig {
        let workers = match (runtime, &self.execution) {
            (Runtime::Embassy, _) | (_, Execution::Inline) => return CryptoPoolConfig::Inline,
            (Runtime::Tokio, Execution::ControlledOne) => NonZeroUsize::MIN,
            (Runtime::Tokio, Execution::ControlledFour) => {
                NonZeroUsize::new(4).expect("four workers")
            }
        };
        CryptoPoolConfig::Controlled(ControlledCrypto::new(
            workers,
            NonZeroUsize::new(CRYPTO_TRACE_CAPACITY).expect("bounded trace"),
        ))
    }
}
pub fn all() -> Vec<Profile> {
    PAIRS
        .into_iter()
        .flat_map(|runtimes| {
            let executions = if runtimes == [Runtime::Embassy, Runtime::Embassy] {
                vec![Execution::Inline]
            } else {
                vec![
                    Execution::Inline,
                    Execution::ControlledOne,
                    Execution::ControlledFour,
                ]
            };
            executions.into_iter().map(move |execution| Profile {
                runtimes: runtimes.clone(),
                execution,
            })
        })
        .collect()
}
