use super::core::{BleAddress, Control, Dialect, Sighting, Transport};

#[allow(async_fn_in_trait)]
pub trait BleBackend {
    const MAX_PEERS: usize;

    type Error: core::fmt::Debug;
    type Link: BleLink<Error = Self::Error>;

    async fn advertise(&mut self) -> Result<(), Self::Error>;
    async fn next_sighting(&mut self) -> Sighting;
    async fn dial(&mut self, address: BleAddress) -> Result<Self::Link, Self::Error>;
    async fn accept(&mut self) -> Result<Self::Link, Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait BleLink {
    type Error: core::fmt::Debug;

    fn dialect(&self) -> Dialect;
    fn address(&self) -> BleAddress;

    async fn control_send(&mut self, msg: &Control) -> Result<(), Self::Error>;
    async fn control_recv(&mut self) -> Result<Control, Self::Error>;

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), Self::Error>;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error>;
}
