use super::{fixture, workloads};
mod artifact;
mod case;
mod execute;
use case::{Case, Scenario};

fn cases(seeds: impl IntoIterator<Item = u64>) -> Vec<Case> {
    let mut result = Vec::new();
    for seed in seeds {
        use workloads::recovery::{BindingFault, Recovery, RestartedNode};
        for recovery in [
            Recovery::MissingUnicastPaths,
            Recovery::DelayedRadioBoot,
            Recovery::PageHandshakeLoss,
            Recovery::RetiredPageRoute,
            Recovery::AdapterReplacement,
            Recovery::FailedHandshake,
            Recovery::RetainedRestart(RestartedNode::Target),
            Recovery::RetainedRestart(RestartedNode::Gateway),
            Recovery::RetainedRestart(RestartedNode::Controller),
            Recovery::ManagedBinding(BindingFault::MissingDevice),
            Recovery::ManagedBinding(BindingFault::CoalescedDownUp),
            Recovery::ManagedBinding(BindingFault::FatalReceive),
        ] {
            result.push(Case {
                version: 1,
                seed,
                topology: fixture::Topology::Shared,
                scenario: Scenario::Recovery(recovery),
            });
        }
        for topology in [
            fixture::Topology::Shared,
            fixture::Topology::Chain,
            fixture::Topology::Asymmetric,
        ] {
            result.push(Case {
                version: 1,
                seed,
                topology,
                scenario: Scenario::Baseline,
            });
        }
        for topology in [fixture::Topology::Shared, fixture::Topology::Chain] {
            for scenario in [Scenario::BroadcastFaults, Scenario::ControlOverlap] {
                result.push(Case {
                    version: 1,
                    seed,
                    topology,
                    scenario,
                });
            }
        }
        for scenario in [
            Scenario::Lifecycle,
            Scenario::SendPressure,
            Scenario::ReceivePressure,
            Scenario::CancelReply,
        ] {
            result.push(Case {
                version: 1,
                seed,
                topology: fixture::Topology::Shared,
                scenario,
            });
        }
        for leg in [workloads::FaultLeg::Request, workloads::FaultLeg::Response] {
            for effect in [
                workloads::FaultEffect::Loss,
                workloads::FaultEffect::Delay,
                workloads::FaultEffect::Duplicate,
                workloads::FaultEffect::Late,
            ] {
                result.push(Case {
                    version: 1,
                    seed,
                    topology: fixture::Topology::Shared,
                    scenario: Scenario::ControlFault { leg, effect },
                });
            }
        }
        for effect in [
            workloads::ResourceFault::Loss,
            workloads::ResourceFault::Delay,
            workloads::ResourceFault::Duplicate,
            workloads::ResourceFault::PartLoss,
            workloads::ResourceFault::ReorderedParts,
        ] {
            result.push(Case {
                version: 1,
                seed,
                topology: fixture::Topology::Shared,
                scenario: Scenario::ResourceFault { effect },
            });
        }
    }
    result
}
fn campaign(family: &str, inputs: Vec<Case>) {
    let directory = artifact::directory(family);
    for (index, case) in inputs.iter().enumerate() {
        artifact::qualify(&directory, index, case);
    }
    println!(
        "{}",
        serde_json::json!({"version": 1, "status": "passed", "cases": inputs.len(), "fresh_runs_per_case": 2, "directory": directory})
    );
}
#[test]
fn routine_halow_campaign() {
    campaign("routine", cases([0, 1, 42, 0x5eed]));
}
#[test]
#[ignore = "expanded exclusively HaLoW qualification"]
fn extended_halow_campaign() {
    campaign("extended", cases(0..32));
}
#[test]
#[ignore = "standalone replay consumes an explicit case file"]
fn replay_case() {
    let path = std::env::var_os("PRNS_HALOW_CASE").expect("explicit HaLoW case path");
    let case: Case =
        serde_json::from_slice(&std::fs::read(path).expect("case bytes")).expect("case schema");
    campaign("replay", vec![case]);
}
#[test]
fn case_schema_rejects_unknown_fields_versions_and_unsupported_topologies() {
    assert!(serde_json::from_str::<Case>(
        r#"{"version":1,"seed":42,"topology":"Chain","scenario":"Baseline","surprise":true}"#
    )
    .is_err());
    let mut case = Case {
        version: 2,
        seed: 42,
        topology: fixture::Topology::Chain,
        scenario: Scenario::Baseline,
    };
    assert_eq!(case.validate(), Err(case::InputError::Version { found: 2 }));
    case.version = 1;
    case.scenario = Scenario::Lifecycle;
    assert_eq!(case.validate(), Err(case::InputError::Topology));
    assert_eq!(cases([42]).len(), 36);
}
