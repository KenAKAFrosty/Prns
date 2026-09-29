use super::*;

mod fixture;
mod inputs;
mod source;

use fixture::{run, Scenario};
use source::{BootId, Read, ReadOutcome};

#[test]
fn explicit_host_entropy_repeats_a_real_node_frame_exchange_byte_for_byte() {
    let first = run(0x31, b"controlled host source", Scenario::Fresh);
    assert!(first
        .trace
        .iter()
        .any(|event| matches!(event, MediumEvent::TransmissionAccepted { .. })));
    assert_eq!(first, run(0x31, b"controlled host source", Scenario::Fresh));
    assert_eq!(first, run(0x31, b"controlled host source", Scenario::Fresh));
}

#[test]
fn changed_host_entropy_and_application_input_change_the_frame_trace() {
    let first = run(0x31, b"one", Scenario::Fresh);
    assert_ne!(first.trace, run(0x41, b"one", Scenario::Fresh).trace);
    assert_ne!(first.trace, run(0x31, b"two", Scenario::Fresh).trace);
}

#[test]
fn receiver_restart_replays_complete_packets_with_a_distinct_boot_source() {
    let run = || {
        run(
            0x31,
            b"across restart",
            Scenario::Restart {
                receiver_seed: 0x71,
            },
        )
    };
    let first = run();
    assert_eq!(
        first.source_calls,
        vec![
            Read::initial(
                BootId {
                    node: 0,
                    generation: 0
                },
                0x31
            ),
            Read::initial(
                BootId {
                    node: 1,
                    generation: 0
                },
                0x32
            ),
            Read::initial(
                BootId {
                    node: 1,
                    generation: 1
                },
                0x71
            ),
        ]
    );
    assert_eq!(
        first.responses,
        [b"across restart".to_vec(), b"across restart".to_vec()]
    );
    assert_eq!(first, run());
    assert_eq!(first, run());
}

#[test]
fn changing_only_restart_entropy_leaves_the_first_exchange_unchanged() {
    let first = run(
        0x31,
        b"same",
        Scenario::Restart {
            receiver_seed: 0x71,
        },
    );
    let changed = run(
        0x31,
        b"same",
        Scenario::Restart {
            receiver_seed: 0x72,
        },
    );
    let boundary = first.checkpoints[0];
    assert_eq!(boundary, changed.checkpoints[0]);
    assert_eq!(first.trace[..boundary], changed.trace[..boundary]);
    assert_ne!(first.trace[boundary..], changed.trace[boundary..]);
    assert_eq!(first.responses, changed.responses);
}

#[test]
fn node_packets_repeat_across_successful_and_failed_periodic_reseeding() {
    for scenario in [
        Scenario::ReseedSuccess { fresh_seed: 0x91 },
        Scenario::ReseedFailure,
    ] {
        let first = run(0x31, b"reseed boundary", scenario);
        let outcome = match scenario {
            Scenario::ReseedSuccess { fresh_seed } => ReadOutcome::Bytes(fresh_seed),
            Scenario::ReseedFailure => ReadOutcome::Unavailable,
            Scenario::Fresh | Scenario::Restart { .. } | Scenario::OwnedInputs { .. } => {
                unreachable!("reseed scenarios only")
            }
        };
        let mut calls = first.source_calls.clone();
        calls.sort_by_key(|read| (read.boot.node, read.attempt));
        assert_eq!(
            calls,
            vec![
                Read::initial(
                    BootId {
                        node: 0,
                        generation: 0
                    },
                    0x31
                ),
                Read {
                    boot: BootId {
                        node: 0,
                        generation: 0
                    },
                    attempt: 2,
                    outcome
                },
                Read::initial(
                    BootId {
                        node: 1,
                        generation: 0
                    },
                    0x32
                ),
                Read {
                    boot: BootId {
                        node: 1,
                        generation: 0
                    },
                    attempt: 2,
                    outcome
                },
            ]
        );
        assert_eq!(first, run(0x31, b"reseed boundary", scenario));
        assert_eq!(first, run(0x31, b"reseed boundary", scenario));
    }
}

#[test]
fn reseed_material_and_failure_are_observable_in_packets() {
    let first = run(0x31, b"same", Scenario::ReseedSuccess { fresh_seed: 0x91 });
    let changed = run(0x31, b"same", Scenario::ReseedSuccess { fresh_seed: 0x92 });
    let failed = run(0x31, b"same", Scenario::ReseedFailure);
    assert_eq!(first.responses, changed.responses);
    assert_eq!(first.responses, failed.responses);
    assert_ne!(first.trace, changed.trace);
    assert_ne!(first.trace, failed.trace);
}

#[test]
fn handle_interface_and_path_sources_replay_through_real_nodes() {
    let first = run(
        0x31,
        b"owned inputs",
        Scenario::OwnedInputs {
            shared_seed: 0x57,
            path_seed: 0xA3,
        },
    );
    let seed = 0x57;
    let mut reference = prns_core::entropy::RuntimeEntropy::try_new(move |output: &mut [u8]| {
        output.fill(seed);
        Ok::<(), core::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {});
    let mut blocks = [[0; 64]; 2];
    for block in &mut blocks {
        reference.fill_random(block);
    }
    assert_eq!(
        first.inputs,
        vec![
            inputs::Observation::Interface {
                boot: BootId {
                    node: 0,
                    generation: 0
                },
                bytes: blocks[0]
            },
            inputs::Observation::Interface {
                boot: BootId {
                    node: 1,
                    generation: 0
                },
                bytes: blocks[0]
            },
            inputs::Observation::Handle { bytes: blocks[1] },
            inputs::Observation::Path {
                boot: BootId {
                    node: 0,
                    generation: 0
                },
                bytes: vec![0xA3; personal_rns::engine::PATH_REQUEST_ID_LEN]
            },
        ]
    );
    assert_eq!(
        first,
        run(
            0x31,
            b"owned inputs",
            Scenario::OwnedInputs {
                shared_seed: 0x57,
                path_seed: 0xA3
            }
        )
    );
    let changed_path = run(
        0x31,
        b"owned inputs",
        Scenario::OwnedInputs {
            shared_seed: 0x57,
            path_seed: 0xB4,
        },
    );
    let changed_shared = run(
        0x31,
        b"owned inputs",
        Scenario::OwnedInputs {
            shared_seed: 0x68,
            path_seed: 0xA3,
        },
    );
    assert_eq!(first.responses, changed_path.responses);
    assert_ne!(first.trace, changed_path.trace);
    assert_ne!(first.responses, changed_shared.responses);
    assert_ne!(first.trace, changed_shared.trace);
}
