#![no_std]
#![forbid(unsafe_code)]

mod future_sizes;
mod lr1110;
mod sx126x;
mod test_support;
mod transcript;

use core::cell::RefCell;
use core::fmt;

use embassy_futures::block_on;
use prns_core::crypto::sha256;

pub use future_sizes::{FutureSizeScenario, SCENARIOS as FUTURE_SIZE_SCENARIOS};
use transcript::HexBytes;
pub use transcript::Transcript;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioError {
    TranscriptFull,
    Sx126x(prns_interfaces_embassy::radios::sx126x::Error),
    Lr1110(prns_interfaces_embassy::radios::lr1110::Error),
    UnexpectedResult,
}

pub struct Evidence {
    transcript: Transcript,
    digest: [u8; 32],
    completed_scenarios: usize,
}

impl Evidence {
    fn new(transcript: Transcript, completed_scenarios: usize) -> Self {
        let digest = sha256(transcript.as_bytes());
        Self {
            transcript,
            digest,
            completed_scenarios,
        }
    }

    #[must_use]
    pub const fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    #[must_use]
    pub const fn completed_scenarios(&self) -> usize {
        self.completed_scenarios
    }
}

impl fmt::Display for Evidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "PRNS_ISA_TRANSCRIPT schema=1 scenarios={} bytes={} digest={} events={}",
            self.completed_scenarios,
            self.transcript.len(),
            HexBytes(self.digest()),
            HexBytes(self.transcript.as_bytes()),
        )
    }
}

pub fn run() -> Result<Evidence, ScenarioError> {
    let transcript = RefCell::new(Transcript::new());
    block_on(sx126x::run(&transcript))?;
    block_on(lr1110::run(&transcript))?;
    let transcript = transcript.into_inner();
    if transcript.overflowed() {
        return Err(ScenarioError::TranscriptFull);
    }
    Ok(Evidence::new(transcript, FUTURE_SIZE_SCENARIOS.len()))
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn shared_scenarios_are_deterministic() -> Result<(), super::ScenarioError> {
        let first = run()?;
        let second = run()?;
        assert_eq!(first.completed_scenarios(), 2);
        assert_eq!(
            first.transcript().as_bytes(),
            second.transcript().as_bytes()
        );
        assert_eq!(first.digest(), second.digest());
        assert_eq!(
            first.digest(),
            &[
                0xc1, 0x92, 0x43, 0xf1, 0x0b, 0xc1, 0x20, 0xa2, 0xcc, 0xfd, 0x88, 0x08, 0x50, 0x45,
                0x11, 0x57, 0xa3, 0xd0, 0x9a, 0xc7, 0x58, 0x11, 0x34, 0x7f, 0x52, 0x67, 0xf3, 0x5e,
                0x1e, 0x53, 0xe8, 0x62,
            ]
        );
        Ok(())
    }
}
