use super::*;
use prns_simulation::halow::{HaLowEvent, HaLowSnapshot};
use std::panic::{catch_unwind, AssertUnwindSafe};
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FailureClass {
    InvalidInput,
    RuntimeInvariant,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Outcome {
    Passed,
    Failed { class: FailureClass, detail: String },
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub outcome: Outcome,
    pub trace: serde_json::Value,
    pub observations: serde_json::Value,
}
pub fn run(case: &Case) -> Report {
    if let Err(error) = case.validate() {
        return Report {
            outcome: Outcome::Failed {
                class: FailureClass::InvalidInput,
                detail: format!("{error:?}"),
            },
            trace: serde_json::Value::Null,
            observations: serde_json::Value::Null,
        };
    }
    let medium = fixture::medium();
    let mut observations = serde_json::Value::Null;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let persistence = match case.scenario {
            Scenario::Recovery(recovery) => recovery.persistence(),
            _ => fixture::Persistence::Disabled,
        };
        let bindings = match case.scenario {
            Scenario::Recovery(recovery) => recovery.bindings(),
            _ => fixture::Bindings::Explicit,
        };
        fixture::with_fixture(
            medium.clone(),
            case.seed,
            case.topology,
            persistence,
            bindings.clone(),
            |lab| {
                match case.scenario {
                    Scenario::Recovery(recovery) => {
                        workloads::recovery::run(lab, recovery, &bindings)
                    }
                    Scenario::Baseline => workloads::baseline(lab, case.topology),
                    Scenario::BroadcastFaults => workloads::broadcast_faults(lab, case.topology),
                    Scenario::ControlOverlap => workloads::overlap(lab),
                    Scenario::Lifecycle => workloads::lifecycle(lab),
                    Scenario::SendPressure => workloads::pressure(lab),
                    Scenario::ReceivePressure => workloads::receive_pressure(lab),
                    Scenario::CancelReply => workloads::cancel_reply(lab),
                    Scenario::ControlFault { leg, effect } => {
                        workloads::faulted_exchange(lab, leg, effect)
                    }
                    Scenario::ResourceFault { effect } => workloads::resource_fault(lab, effect),
                }
                let metrics = lab.drained_metrics();
                observations = serde_json::json!({"calls": lab.calls.borrow().iter().map(|call| serde_json::json!({"controller": call.controller.as_bytes(), "payload": call.payload})).collect::<Vec<_>>(), "announces": lab.announces.borrow().iter().map(|seen| serde_json::json!({"node": seen.node, "destination": seen.destination.as_bytes()})).collect::<Vec<_>>(), "metrics": metrics, "measurements":lab.measurements});
            },
        );
    }));
    let outcome = match result {
        Ok(()) => Outcome::Passed,
        Err(panic) => Outcome::Failed {
            class: FailureClass::RuntimeInvariant,
            detail: panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    panic
                        .downcast_ref::<&str>()
                        .map(|message| (*message).to_owned())
                })
                .unwrap_or_else(|| "non-text invariant panic".to_owned()),
        },
    };
    Report {
        outcome,
        trace: trace(medium.snapshot()),
        observations,
    }
}
fn trace(snapshot: HaLowSnapshot) -> serde_json::Value {
    use personal_rns::wifi_halow::Destination;
    let events: Vec<_> = snapshot.events.iter().map(|event| match event {
        HaLowEvent::Transmitted { ordinal, from, destination, at, bytes } => serde_json::json!({"event": "transmitted", "ordinal": ordinal, "from": from.get(), "destination": match destination { Destination::Broadcast => serde_json::json!({"kind":"broadcast"}), Destination::Peer(peer) => serde_json::json!({"kind":"unicast", "mac":peer.address().octets()}) }, "at":at.get(), "bytes":bytes}),
        HaLowEvent::Delivery { ordinal, to, copy, at, outcome } => serde_json::json!({"event":"delivery", "ordinal":ordinal, "to":to.get(), "copy":copy, "at":at.get(), "outcome":format!("{outcome:?}")}),
        HaLowEvent::Scheduled { ordinal, to, copy, at } => serde_json::json!({"event":"scheduled", "ordinal":ordinal, "to":to.get(), "copy":copy, "at":at.get()}),
        HaLowEvent::Attached { radio, mac } => serde_json::json!({"event":"attached", "radio":radio.get(), "mac":mac.address().octets()}),
        HaLowEvent::Detached { radio } => serde_json::json!({"event":"detached", "radio":radio.get()}),
        HaLowEvent::Path { from, to, state, at } => serde_json::json!({"event":"path", "from":from.get(), "to":to.get(), "state":format!("{state:?}"), "at":at.get()}),
        HaLowEvent::SendBehavior { radio, behavior, at } => serde_json::json!({"event":"send_behavior", "radio":radio.get(), "behavior":format!("{behavior:?}"), "at":at.get()}),
        HaLowEvent::ReceiveBehavior { radio, behavior, at } => serde_json::json!({"event":"receive_behavior", "radio":radio.get(), "behavior":format!("{behavior:?}"), "at":at.get()}),
        HaLowEvent::FaultArmed { fault, at } => serde_json::json!({"event":"fault_armed", "from":fault.from.get(), "destination":format!("{:?}", fault.destination), "action":format!("{:?}", fault.action), "at":at.get()}),
        HaLowEvent::Injected { from, to, at, bytes, outcome } => serde_json::json!({"event":"injected", "source":from.address().octets(), "to":to.get(), "at":at.get(), "bytes":bytes, "outcome":format!("{outcome:?}")}),
    }).collect();
    serde_json::json!({"radios":snapshot.radios, "queued":snapshot.queued, "pending":snapshot.pending, "armed_faults":snapshot.armed_faults, "peak_queued":snapshot.peak_queued, "peak_pending":snapshot.peak_pending, "retention":format!("{:?}", snapshot.retention), "events":events})
}
