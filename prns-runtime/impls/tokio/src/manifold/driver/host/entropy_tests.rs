use std::{cell::Cell, rc::Rc};

use super::*;
use prns_core::entropy::ReseedHealth;

const RESEED_WINDOW: usize = 64 * 1024;

#[derive(Debug, PartialEq)]
struct SourceUnavailable;

struct ScriptedSource {
    calls: Rc<Cell<usize>>,
}

impl EntropySource for ScriptedSource {
    type Error = SourceUnavailable;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        match call {
            1 => output.fill(0x57),
            2 => return Err(SourceUnavailable),
            _ => output.fill(0xA3),
        }
        Ok(())
    }
}

fn stream() -> (RuntimeEntropy<ScriptedSource>, Rc<Cell<usize>>) {
    let calls = Rc::new(Cell::new(0));
    let entropy = RuntimeEntropy::try_new(ScriptedSource {
        calls: Rc::clone(&calls),
    })
    .expect("initial scripted seed succeeds");
    (entropy, calls)
}

#[tokio::test(start_paused = true)]
async fn moving_a_partly_consumed_stream_preserves_position_and_clock() {
    let (mut entropy, calls) = stream();
    let (mut reference, _) = stream();
    let mut prefix = [0; 73];
    entropy.fill_random(&mut prefix);
    reference.fill_random(&mut [0; 73]);
    let mut host = TokioHost::with_runtime_entropy(InstantMillis(900), entropy);
    let mut actual = [0; 129];
    let mut expected = [0; 129];
    host.fill_random(&mut actual);
    reference.fill_random(&mut expected);
    assert_eq!(
        (actual, calls.get(), host.now()),
        (expected, 1, InstantMillis(900))
    );
    tokio::time::advance(Duration::from_millis(7)).await;
    host.fill_random(&mut []);
    assert_eq!((host.now(), calls.get()), (InstantMillis(907), 1));
}

#[test]
fn supplied_source_owns_failed_reseed_and_recovery_without_os_fallback() {
    let (entropy, calls) = stream();
    let (mut reference, reference_calls) = stream();
    let mut host = TokioHost::with_runtime_entropy(InstantMillis(0), entropy);
    for (length, expected_calls, health) in [
        (RESEED_WINDOW, 1, ReseedHealth::Healthy),
        (RESEED_WINDOW, 2, ReseedHealth::Deferred),
        (127, 3, ReseedHealth::Healthy),
    ] {
        let mut actual = vec![0; length];
        let mut expected = vec![0; length];
        host.fill_random(&mut actual);
        reference.fill_random(&mut expected);
        assert_eq!(
            (actual, calls.get(), host.entropy.reseed_health()),
            (expected, expected_calls, health)
        );
        assert_eq!(reference_calls.get(), expected_calls);
    }
}

#[test]
fn resetting_only_the_timeline_never_rewinds_entropy() {
    let (entropy, calls) = stream();
    let (mut reference, _) = stream();
    let mut host = TokioHost::with_runtime_entropy(InstantMillis(0), entropy);
    let mut actual = [0; 128];
    let mut expected = [0; 128];
    host.fill_random(&mut actual[..64]);
    host.set_timeline_origin(InstantMillis(100));
    host.fill_random(&mut actual[64..]);
    reference.fill_random(&mut expected[..64]);
    reference.fill_random(&mut expected[64..]);
    assert_eq!((actual, calls.get()), (expected, 1));
}
