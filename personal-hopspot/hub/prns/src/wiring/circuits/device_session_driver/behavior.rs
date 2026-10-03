use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - A route that exceeds the bounded command slots reports the rejected command
        - Abandoning a running idle session stops admission and releases its link
        - Arbitrary intent histories match physical connection and inventory ownership
        - Automatic connection pages refresh and shutdown preserve the registry
        - Cancellations remove only matching work and preserve close order
        - Close cancels connect and inventory but never another close or connection
        - Command backlog defers intents and last sender closure still drains pending work
        - Command capacity invariants propagate out of the circuit
        - Disconnect interrupts inventory and another connection can then start
        - Dropping an unpolled runtime stops admission without executing device work
        - Execution preserves success and invariant settlement through the same observer
        - Idle mio poll is woken by last handle drop stop guard and owned future waker
        - Mailbox capacity stop and disconnection preserve rejected intents
        - Observer panics drain the worker and return the join error
        - Readiness is retained and interrupted or spurious polls retry without losing failures
        - Ready work is not starved by intents and shutdown preempts both
        - Stale reactions and foreign completions do not mutate the registry
        - Worker invariants terminate conduction and return registry ownership
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
