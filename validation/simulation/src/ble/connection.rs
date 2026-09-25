use std::sync::Arc;

use tokio::sync::watch;

use super::BleAddress;

pub(super) struct Connection {
    addresses: [BleAddress; 2],
    closed: watch::Sender<bool>,
}

impl Connection {
    pub(super) fn new(first: BleAddress, second: BleAddress) -> Self {
        let (closed, _) = watch::channel(false);
        Self {
            addresses: [first, second],
            closed,
        }
    }

    pub(super) fn connects(&self, address: BleAddress) -> bool {
        self.addresses.contains(&address)
    }

    pub(super) fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }

    pub(super) fn close(&self) -> bool {
        self.closed.send_if_modified(|closed| {
            let changed = !*closed;
            *closed = true;
            changed
        })
    }
}

pub(super) struct ConnectionEndpoint {
    pub(super) connection: Arc<Connection>,
}

impl Drop for ConnectionEndpoint {
    fn drop(&mut self) {
        let _ = self.connection.close();
    }
}
