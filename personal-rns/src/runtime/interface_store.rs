use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::engine::InterfaceCounts;
use crate::interfaces::InterfaceId;

#[derive(Clone)]
pub struct InterfaceStore {
    inner: Arc<Shared>,
}

struct Shared {
    counts: Mutex<HashMap<InterfaceId, InterfaceCounts>>,
    epoch: watch::Sender<u64>,
}

impl InterfaceStore {
    pub(crate) fn new() -> Self {
        let (epoch, _) = watch::channel(0);
        Self {
            inner: Arc::new(Shared {
                counts: Mutex::new(HashMap::new()),
                epoch,
            }),
        }
    }

    pub(crate) fn set(&self, interface: InterfaceId, counts: InterfaceCounts) {
        if let Ok(mut map) = self.inner.counts.lock() {
            map.insert(interface, counts);
        }
    }

    pub(crate) fn bump(&self) {
        self.inner
            .epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    #[must_use]
    pub fn counts(&self, interface: InterfaceId) -> InterfaceCounts {
        self.inner
            .counts
            .lock()
            .ok()
            .and_then(|map| map.get(&interface).copied())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            rx: self.inner.epoch.subscribe(),
        }
    }
}

pub struct Subscription {
    rx: watch::Receiver<u64>,
}

impl Subscription {
    pub async fn changed(&mut self) {
        let _ = self.rx.changed().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::INTERFACE_ID_LEN;

    #[tokio::test]
    async fn set_reads_back_and_bump_wakes_a_live_subscription() {
        let store = InterfaceStore::new();
        let interface = InterfaceId::new([7; INTERFACE_ID_LEN]);
        let mut subscription = store.subscribe();

        assert_eq!(store.counts(interface), InterfaceCounts::default());

        store.set(
            interface,
            InterfaceCounts {
                destinations: 3,
                links: 1,
                transported_links: 0,
            },
        );
        store.bump();

        subscription.changed().await;
        assert_eq!(store.counts(interface).destinations, 3);
    }
}
