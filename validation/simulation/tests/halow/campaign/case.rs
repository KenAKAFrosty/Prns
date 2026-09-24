use super::*;
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub version: u8,
    pub seed: u64,
    pub topology: fixture::Topology,
    pub scenario: Scenario,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Scenario {
    Baseline,
    Recovery(workloads::recovery::Recovery),
    BroadcastFaults,
    ControlOverlap,
    Lifecycle,
    SendPressure,
    ReceivePressure,
    CancelReply,
    ControlFault {
        leg: workloads::FaultLeg,
        effect: workloads::FaultEffect,
    },
    ResourceFault {
        effect: workloads::ResourceFault,
    },
}
#[derive(Debug, PartialEq, Eq)]
pub enum InputError {
    Version { found: u8 },
    Topology,
}
impl Case {
    pub fn validate(&self) -> Result<(), InputError> {
        if self.version != 1 {
            return Err(InputError::Version {
                found: self.version,
            });
        }
        let supported = match self.scenario {
            Scenario::Baseline => true,
            Scenario::BroadcastFaults | Scenario::ControlOverlap => {
                self.topology != fixture::Topology::Asymmetric
            }
            Scenario::Recovery(_)
            | Scenario::Lifecycle
            | Scenario::SendPressure
            | Scenario::ReceivePressure
            | Scenario::CancelReply
            | Scenario::ControlFault { .. }
            | Scenario::ResourceFault { .. } => self.topology == fixture::Topology::Shared,
        };
        if supported {
            Ok(())
        } else {
            Err(InputError::Topology)
        }
    }
}
