use super::super::{
    fixture::{self, Triple, PRIMARY, TARGET},
    workloads,
};
use super::*;
use crate::remote_control::adapter::Handle;
use std::{
    cell::Cell,
    panic::{catch_unwind, AssertUnwindSafe},
};

pub fn metrics(triple: &mut Triple<'_, '_>, node: usize) -> Metrics {
    let Handle::Tokio(handle) = triple.nodes[node].handle.clone() else {
        return Metrics::Unavailable;
    };
    let snapshot = triple.complete(async move {
        handle
            .metrics_snapshot()
            .await
            .expect("actual runtime snapshot")
    });
    assert_eq!(
        snapshot.engine.resources.incoming.active_rows, 0,
        "incoming resource ownership released"
    );
    assert_eq!(
        snapshot.engine.resources.outgoing.active_rows, 0,
        "outgoing resource ownership released"
    );
    assert_eq!(
        snapshot.engine.resources.pending_depth, 0,
        "resource admission queue drained"
    );
    if let Some(crypto) = snapshot.crypto {
        assert_eq!(crypto.queue_depth, 0, "worker result accounting drained");
        assert_eq!(
            crypto.packet_verdicts_owed, 0,
            "packet verdict ownership drained"
        );
    }
    Metrics::Available(
        snapshot
            .reliability
            .operations
            .iter()
            .filter(|(_, _, count)| *count != 0)
            .map(|(operation, outcome, count)| {
                (format!("{operation:?}"), format!("{outcome:?}"), count)
            })
            .collect(),
    )
}
fn apply(triple: &mut Triple<'_, '_>, action: &Action) {
    match action {
        Action::Overlap { marker } => workloads::overlap(triple, *marker),
        Action::HeldWorker => workloads::worker(triple, workloads::WorkerDisposition::Release),
        Action::CancelTransfer => {
            workloads::worker(triple, workloads::WorkerDisposition::CloseLink)
        }
        Action::RetireWorker => workloads::worker(triple, workloads::WorkerDisposition::Retire),
        Action::WindowPressure => workloads::window_pressure(triple),
        Action::RefusedResponse => workloads::refused_response(triple),
        Action::LoseReply => workloads::reply_fault(triple, workloads::ReplyFault::Lose),
        Action::CancelReply => workloads::reply_fault(triple, workloads::ReplyFault::Cancel),
        Action::RestartController => triple.restart(PRIMARY),
        Action::RestartTarget => triple.restart(TARGET),
        Action::Inventory => workloads::inventory(triple),
        Action::Watch => workloads::watch(triple),
        Action::ByteStream { marker } => workloads::byte_stream(triple, *marker),
        Action::PersistenceGate => workloads::persistence_gate(triple),
    }
}
fn verify_counts(action: &Action, before: &Metrics, after: &Metrics) {
    use prns_runtime_tokio::runtime::{RuntimeOperation, RuntimeOperationOutcome};
    let expected = match action {
        Action::LoseReply | Action::CancelReply => Some((
            RuntimeOperation::SendRequest,
            RuntimeOperationOutcome::Timeout,
        )),
        Action::RefusedResponse => Some((
            RuntimeOperation::SendRequest,
            RuntimeOperationOutcome::ResponseTooLarge,
        )),
        Action::WindowPressure => Some((
            RuntimeOperation::SendToChannel,
            RuntimeOperationOutcome::Backpressure,
        )),
        _ => None,
    };
    let (Some((operation, outcome)), Metrics::Available(before), Metrics::Available(after)) =
        (expected, before, after)
    else {
        return;
    };
    let operation = format!("{operation:?}");
    let outcome = format!("{outcome:?}");
    let count = |values: &[(String, String, u64)]| {
        values
            .iter()
            .find_map(|(op, result, count)| {
                (op == &operation && result == &outcome).then_some(*count)
            })
            .unwrap_or(0)
    };
    assert_eq!(
        count(after) - count(before),
        1,
        "one public outcome matches one owning runtime counter"
    );
}
fn panic_detail(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(detail) = panic.downcast_ref::<String>() {
        return detail.clone();
    }
    if let Some(detail) = panic.downcast_ref::<&str>() {
        return (*detail).to_owned();
    }
    "non-text panic".to_owned()
}
pub fn run(case: &Case) -> Run {
    if let Err(error) = case.validate() {
        return Run::Failed {
            failure: Failure {
                class: FailureClass::InvalidInput,
                action: None,
                stage: 0,
                detail: format!("{error:?}"),
            },
            report: None,
        };
    }
    let stage = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        fixture::with_triple(case.profile.clone(), case.seed, |triple| {
            let mut observations = Vec::new();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                for (index, action) in case.actions.iter().enumerate() {
                    stage.set(index);
                    let before = metrics(triple, PRIMARY);
                    apply(triple, action);
                    let metrics = std::array::from_fn(|node| metrics(triple, node));
                    verify_counts(action, &before, &metrics[PRIMARY]);
                    observations.push(Observation {
                        action: action.clone(),
                        generations: triple.generations,
                        app_calls: triple.messages.0.borrow().len(),
                        metrics,
                    });
                }
            }));
            (
                outcome.map_err(|panic| panic_detail(panic.as_ref())),
                observations,
            )
        })
    }));
    let failure = |detail| Failure {
        class: FailureClass::Runtime,
        action: case.actions.get(stage.get()).cloned(),
        stage: stage.get(),
        detail,
    };
    match result {
        Ok(((outcome, observations), trace)) => {
            let report = Report {
                observations,
                trace: serde_json::json!({ "wire": trace.wire, "persistence": trace.persistence, "events": trace.events.iter().map(|(node,generation,events)| serde_json::json!({"node":node,"generation":generation,"events":events.iter().map(|event| format!("{event:?}")).collect::<Vec<_>>()})).collect::<Vec<_>>(), "crypto": trace.crypto.iter().map(|(node,generation,events)| serde_json::json!({"node":node,"generation":generation,"events":events.iter().map(|event| format!("{event:?}")).collect::<Vec<_>>()})).collect::<Vec<_>>(), "retained_static": format!("{:?}", trace.retained_static) }),
            };
            match outcome {
                Ok(()) => Run::Passed(report),
                Err(detail) => Run::Failed {
                    failure: failure(detail),
                    report: Some(report),
                },
            }
        }
        Err(panic) => Run::Failed {
            failure: failure(panic_detail(panic.as_ref())),
            report: None,
        },
    }
}
