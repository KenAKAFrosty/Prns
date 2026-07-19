use std::collections::BTreeSet;
use std::future;
use std::time::Duration;

use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::runtime::PrnsNodeHandle;
use tokio::sync::watch;

pub(super) async fn wait(
    handle: &PrnsNodeHandle,
    expected: watch::Receiver<BTreeSet<InterfaceId>>,
    enabled: bool,
) -> InterfaceId {
    if !enabled {
        return future::pending().await;
    }
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        interval.tick().await;
        let current = handle
            .interfaces()
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot.connection))
            .collect::<Vec<_>>();
        let expected = expected.borrow().iter().copied().collect::<Vec<_>>();
        if let Some(failed) = first_failure(&expected, &current) {
            return failed;
        }
    }
}

fn first_failure(
    expected: &[InterfaceId],
    current: &[(InterfaceId, ConnectionState)],
) -> Option<InterfaceId> {
    current
        .iter()
        .find_map(|(id, connection)| (*connection == ConnectionState::Failed).then_some(*id))
        .or_else(|| {
            expected
                .iter()
                .copied()
                .find(|expected| !current.iter().any(|(current, _)| current == expected))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_covers_failed_and_departed_initial_interfaces() {
        let first = InterfaceId::new([1; 8]);
        let second = InterfaceId::new([2; 8]);
        let expected = [first, second];

        assert_eq!(
            first_failure(
                &expected,
                &[
                    (first, ConnectionState::Connected),
                    (second, ConnectionState::Reconnecting),
                ],
            ),
            None
        );
        assert_eq!(
            first_failure(
                &expected,
                &[
                    (first, ConnectionState::Connected),
                    (second, ConnectionState::Failed),
                ],
            ),
            Some(second)
        );
        assert_eq!(
            first_failure(&expected, &[(first, ConnectionState::Connected)]),
            Some(second)
        );
    }
}
