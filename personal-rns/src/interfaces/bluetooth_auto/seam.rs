use super::core::{BleAddress, Control, Dialect, L2capPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Dialed,
    Accepted,
}

pub enum BleEvent<L> {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(L),
    LinkReady {
        link: L,
        origin: Origin,
        peer_rssi: Option<i8>,
    },
    DialFailed {
        address: BleAddress,
    },
}

#[allow(async_fn_in_trait)]
pub trait BleBackend {
    const MAX_PEERS: usize;

    type Error: core::fmt::Debug;
    type Link: BleLink<Error = Self::Error>;

    /// Why this backend cannot safely run (e.g. a host policy that would prompt every nearby peer),
    /// or `None` to start normally. The supervisor surfaces a blocked backend as a `Failed` interface
    /// instead of bringing the radio up; most backends never block and keep the default.
    fn blocked(&self) -> Option<&'static str> {
        None
    }

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Self::Error>;
    /// Scan for peers advertising our service so the supervisor can dial them. The supervisor gates
    /// this on capacity, exactly like advertising. A backend that scans autonomously (or cannot
    /// scan) keeps the default no-op.
    async fn set_scanning(&mut self, _enabled: bool) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn next_event(&mut self) -> BleEvent<Self::Link>;
    async fn dial(&mut self, address: BleAddress);
    async fn on_link_closed(&mut self, _address: BleAddress) {}
}

#[allow(async_fn_in_trait)]
pub trait BleLink {
    type Error: core::fmt::Debug;
    type Source: BleSource<Error = Self::Error>;
    type Sink: BleSink<Error = Self::Error>;

    fn dialect(&self) -> Dialect;
    fn address(&self) -> BleAddress;

    async fn control_send(&mut self, msg: &Control) -> Result<(), Self::Error>;
    async fn control_recv(&mut self) -> Result<Control, Self::Error>;

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Self::Error>;

    fn into_data(self) -> (Self::Source, Self::Sink);
}

#[allow(async_fn_in_trait)]
pub trait BleSource {
    type Error: core::fmt::Debug;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait BleSink {
    type Error: core::fmt::Debug;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
}

#[cfg(feature = "embassy-seam")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(feature = "embassy-seam")]
use embassy_sync::signal::Signal;

#[cfg(feature = "embassy-seam")]
pub struct LinkFuse<'a, M: RawMutex> {
    dead: &'a Signal<M, ()>,
    armed: bool,
}

#[cfg(feature = "embassy-seam")]
impl<'a, M: RawMutex> LinkFuse<'a, M> {
    #[must_use]
    pub fn new(dead: &'a Signal<M, ()>) -> Self {
        Self { dead, armed: true }
    }

    #[must_use]
    pub fn signal(&self) -> &'a Signal<M, ()> {
        self.dead
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(feature = "embassy-seam")]
impl<M: RawMutex> Drop for LinkFuse<'_, M> {
    fn drop(&mut self) {
        if self.armed {
            self.dead.signal(());
        }
    }
}

#[cfg(all(test, feature = "embassy-seam"))]
mod link_fuse_tests {
    use super::LinkFuse;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::signal::Signal;

    #[test]
    fn an_armed_fuse_signals_teardown_when_dropped() {
        let dead = Signal::<CriticalSectionRawMutex, ()>::new();
        {
            let _fuse = LinkFuse::new(&dead);
        }
        assert!(dead.signaled());
    }

    #[test]
    fn a_disarmed_fuse_leaves_the_link_alive() {
        let dead = Signal::<CriticalSectionRawMutex, ()>::new();
        {
            let mut fuse = LinkFuse::new(&dead);
            fuse.disarm();
        }
        assert!(!dead.signaled());
    }

    #[test]
    fn the_fuse_signals_at_most_the_teardown_it_owns() {
        let dead = Signal::<CriticalSectionRawMutex, ()>::new();
        {
            let mut fuse = LinkFuse::new(&dead);
            let watched = fuse.signal();
            fuse.disarm();
            assert!(!watched.signaled());
        }
        assert!(!dead.signaled());
    }
}
