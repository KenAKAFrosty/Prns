use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Boot,
    Discovery,
    InitialTraffic,
    IsolatedTraffic,
    RecoveryTraffic,
    Shutdown,
}

impl Phase {
    pub(super) const ALL: [Self; 6] = [
        Self::Boot,
        Self::Discovery,
        Self::InitialTraffic,
        Self::IsolatedTraffic,
        Self::RecoveryTraffic,
        Self::Shutdown,
    ];
}

#[test]
fn phase_observation_preserves_the_complete_replay() {
    let inputs = Inputs {
        scheduling: ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(7),
        },
        host_seed: 11,
        payload_marker: 42,
    };
    let expected = run::<8>(inputs);
    let mut observed = Vec::new();
    let actual = run_observed::<8>(inputs, |phase| observed.push(phase));
    assert_eq!(observed, Phase::ALL);
    assert_replay(&actual, &expected);
}
