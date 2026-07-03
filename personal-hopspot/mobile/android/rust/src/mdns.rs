//! The Android WiFi/LAN mDNS conduit. NsdManager (the OS's Bonjour) runs on the Kotlin side: it
//! registers this node's rendezvous as a `_reticulum._tcp` service and resolves the peers it finds,
//! handing each resolved endpoint to the JNI layer. The reactor's WiFi/LAN supervisor consumes those
//! sightings through `AutoWifi::with_mdns`, dialing each peer's rendezvous — the same protocol the
//! macOS and iOS Bonjour backends speak, so Android, iOS, and the desktops discover one another over
//! mDNS even where raw multicast cannot cross (iOS).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// The handle the JNI layer holds (cheap-clone): it pushes each peer endpoint NsdManager
/// resolves. The engine's clone takes the receiver once to feed the supervisor's mDNS channel.
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

    /// JNI side: a peer's rendezvous endpoint NsdManager just resolved. Dropped silently once the
    /// receiver is gone (the node is shutting down).
    pub fn sighting(&self, addr: SocketAddr) {
        let _ = self.sightings.send(addr);
    }

    /// The engine takes the receiver exactly once, to hand to `AutoWifi::with_mdns`. `None` on any
    /// later call.
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
