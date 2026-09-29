use super::*;
use crate::scenario::TIMELINE_ORIGIN;
use personal_rns::engine::InstantMillis;
use prns_simulation::EndpointId;

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Clocks(Vec<(InstantMillis, InstantMillis)>),
    Response { node: usize, bytes: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq)]
enum TraceEntry {
    Frame {
        ordinal: TransmissionOrdinal,
        from: EndpointId,
        at: SimulationTick,
        length: usize,
    },
    Medium(MediumEvent),
}

#[derive(Debug, PartialEq, Eq)]
struct Transcript {
    observations: Vec<Observation>,
    trace: Vec<TraceEntry>,
}

fn record_clocks(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::LiveNode],
) -> Observation {
    let mut expected = BTreeMap::new();
    for (node, live) in nodes.iter().enumerate() {
        let clock = live.control.clock.clone();
        let origin = live.control.origin;
        let task = runner
            .insert(async move {
                Completion::Clock {
                    node,
                    elapsed: clock.now().duration_since(origin),
                }
            })
            .unwrap_or_else(|error| unreachable!("bounded clock observer: {error}"));
        expected.insert(task, node);
    }
    let completed = settle(runner);
    assert_eq!(completed.len(), nodes.len());
    let mut clocks = BTreeMap::new();
    for (task, completion) in completed {
        let Completion::Clock { node, elapsed } = completion else {
            unreachable!("only clock observers complete")
        };
        assert_eq!(expected.remove(&task), Some(node));
        let origin = nodes[node].control.origin;
        assert!(clocks
            .insert(node, (origin, InstantMillis(origin.0 + elapsed.0)))
            .is_none());
    }
    assert!(expected.is_empty());
    Observation::Clocks(clocks.into_values().collect())
}

fn exchange(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::LiveNode],
    payload: &[u8],
) -> Observation {
    let established = link(runner, nodes, 0, 1);
    let task = request(
        runner,
        0,
        nodes[0].control.handle.clone(),
        established,
        payload.to_vec(),
    );
    let mut completed = settle(runner);
    assert_eq!(completed.len(), 1);
    let (observed, completion) = completed.remove(0);
    assert_eq!(observed, task);
    let Completion::Response { node, bytes } = completion else {
        unreachable!("replay request must return a response")
    };
    assert_eq!((node, bytes.as_slice()), (0, payload));
    Observation::Response { node, bytes }
}

fn run(payload: &[u8]) -> Transcript {
    let mut observations = Vec::new();
    let mut trace = Vec::new();
    with_fleet(FaultPlan::none(), |runner, medium, nodes| {
        observations.push(record_clocks(runner, nodes));
        observations.push(exchange(runner, nodes, payload));
        let boundary = tick(7);
        assert!(runner.advance_to_next_event(boundary).is_ok());
        assert_eq!(medium.now(), boundary);
        assert!(settle(runner).is_empty());
        restart(runner, medium, nodes, 1);
        connect(medium, nodes, 0, 1);
        observations.push(record_clocks(runner, nodes));
        observations.push(exchange(runner, nodes, payload));
        let snapshot = medium.trace();
        assert_eq!(snapshot.discarded_events, 0);
        trace = snapshot
            .events
            .into_iter()
            .map(|event| match event {
                MediumEvent::TransmissionAccepted {
                    ordinal,
                    from,
                    at,
                    frame,
                } => TraceEntry::Frame {
                    ordinal,
                    from,
                    at,
                    length: frame.len(),
                },
                other => TraceEntry::Medium(other),
            })
            .collect();
    });
    Transcript {
        observations,
        trace,
    }
}

#[test]
fn normalized_real_node_restart_transcript_repeats_without_wall_time_or_packet_bytes() {
    let payload = b"repeatable application value";
    let first = run(payload);
    let later = InstantMillis(TIMELINE_ORIGIN.0 + 7);
    assert_eq!(
        first.observations,
        vec![
            Observation::Clocks(vec![
                (TIMELINE_ORIGIN, TIMELINE_ORIGIN);
                fixture::NODE_COUNT
            ]),
            Observation::Response {
                node: 0,
                bytes: payload.to_vec()
            },
            Observation::Clocks(vec![
                (TIMELINE_ORIGIN, later),
                (later, later),
                (TIMELINE_ORIGIN, later),
                (TIMELINE_ORIGIN, later),
            ]),
            Observation::Response {
                node: 0,
                bytes: payload.to_vec()
            },
        ]
    );
    assert!(!first.trace.is_empty());
    assert_eq!(first, run(payload));
    assert_eq!(first, run(payload));
}

#[test]
fn normalized_transcript_retains_application_values() {
    let first = run(b"one");
    let second = run(b"two");
    assert_ne!(first.observations, second.observations);
}
