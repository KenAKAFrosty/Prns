use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub struct AndroidMdnsBridge {
    sightings: UnboundedSender<SocketAddr>,
    receiver: Arc<Mutex<Option<UnboundedReceiver<SocketAddr>>>>,
}

impl Clone for AndroidMdnsBridge {
    fn clone(&self) -> Self {
        Self {
            sightings: self.sightings.clone(),
            receiver: Arc::clone(&self.receiver),
        }
    }
}

impl AndroidMdnsBridge {
    #[must_use]
    pub fn new() -> Self {
        let (sightings, receiver) = unbounded_channel();
        Self {
            sightings,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub fn sighting(&self, addr: SocketAddr) {
        let _ = self.sightings.send(addr);
    }

    #[must_use]
    pub fn take_receiver(&self) -> Option<UnboundedReceiver<SocketAddr>> {
        self.receiver.lock().ok().and_then(|mut guard| guard.take())
    }
}

impl Default for AndroidMdnsBridge {
    fn default() -> Self {
        Self::new()
    }
}
