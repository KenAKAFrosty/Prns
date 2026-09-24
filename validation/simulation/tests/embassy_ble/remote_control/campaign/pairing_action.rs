use super::super::tests::Pair;
use super::*;

pub(super) fn run(pair: &mut Pair<'_, '_>, approval: &Approval) {
    let target = pair.nodes[TARGET].handle.clone();
    pair.complete(async move { target.close_pairing().await });
    pair.messages.1.borrow_mut().clear();
    let target = pair.nodes[TARGET].handle.clone();
    let opened = pair.complete(async move { target.open_pairing().await });
    let opened = pairing::observed_offer(&pair.messages.1, opened);
    let controller = pair.nodes[CONTROLLER].handle.clone();
    pair.complete(async move { controller.initiate(opened).await });
    let mut events = std::mem::take(&mut *pair.messages.1.borrow_mut());
    let target = events
        .iter()
        .position(|event| matches!(event, pairing::Observation::Target(_)))
        .expect("target confirmation");
    let pairing::Observation::Target(target) = events.remove(target) else {
        unreachable!("target confirmation")
    };
    let controller = events
        .iter()
        .position(|event| matches!(event, pairing::Observation::Controller(_)))
        .expect("controller confirmation");
    let pairing::Observation::Controller(controller) = events.remove(controller) else {
        unreachable!("controller confirmation")
    };
    assert_eq!(controller.confirmation(), target.confirmation());
    match approval {
        Approval::Accept => {
            let handle = pair.nodes[TARGET].handle.clone();
            pair.complete(async move { handle.approve_target(target).await });
            let handle = pair.nodes[CONTROLLER].handle.clone();
            pair.complete(async move { handle.approve_controller(controller).await });
            assert_eq!(
                *pair.messages.1.borrow(),
                [
                    pairing::Observation::TargetPersisted,
                    pairing::Observation::ControllerPersisted
                ]
            );
        }
        Approval::RejectController => {
            let handle = pair.nodes[CONTROLLER].handle.clone();
            pair.complete(async move { handle.reject_controller(controller).await });
        }
        Approval::RejectTarget => {
            let handle = pair.nodes[TARGET].handle.clone();
            pair.complete(async move { handle.reject_target(target).await });
        }
        Approval::Expire => {
            let horizon = crate::tick(pair.tasks.snapshot().tick.get() + 10_001);
            while pair.tasks.snapshot().tick < horizon {
                pair.tasks
                    .advance_to_next_wake(horizon)
                    .expect("expiration clock");
                pair.tasks.settle();
            }
            assert!(
                pair.messages.1.borrow().is_empty(),
                "expiry publishes no authority"
            );
        }
    }
    pair.reconnect();
}
