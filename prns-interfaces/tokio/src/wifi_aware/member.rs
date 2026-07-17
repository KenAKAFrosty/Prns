use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::framed_stream;
use prns_core::interfaces::tcp::core as tcp_core;
use prns_core::interfaces::wifi_aware::core;
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::reactor::airtime::AirtimeLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::reactor::throughput::ThroughputLedger;

pub struct WifiAwareMember<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    bitrate: BitrateBps,
    status: TokioInterfaceStatus,
    closed: Option<mpsc::UnboundedSender<InterfaceId>>,
}

impl<S> WifiAwareMember<S> {
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S, bitrate: BitrateBps) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiAwarePeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            bitrate,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
            closed: None,
        }
    }

    #[must_use]
    pub fn report_close_to(mut self, sink: mpsc::UnboundedSender<InterfaceId>) -> Self {
        self.closed = Some(sink);
        self
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for WifiAwareMember<S> {
    const HW_MTU: usize = core::WIFI_AWARE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::WifiAwarePeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.bitrate)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers = framed_stream::FramedBuffers::<
            framed_stream::HdlcFraming,
            { tcp_core::READ_BUF_LEN },
            { tcp_core::FRAMED_LEN },
        >::new();
        framed_stream::serve::<
            framed_stream::HdlcFraming,
            { tcp_core::READ_BUF_LEN },
            { tcp_core::FRAMED_LEN },
            _,
            _,
        >(
            stream,
            &mut buffers,
            &mut seam,
            &mut framed_stream::WireMeters {
                status: &self.status,
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: self.bitrate,
                started,
            },
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
        if let Some(sink) = self.closed.take() {
            let _ = sink.send(self.id);
            std::future::pending::<()>().await;
        }
    }
}

impl<S> prns_core::interfaces::ReportsStatus for WifiAwareMember<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duplex_member(
        tag: &[u8],
    ) -> (
        WifiAwareMember<tokio::io::DuplexStream>,
        tokio::io::DuplexStream,
    ) {
        let (near, far) = tokio::io::duplex(1024);
        (
            WifiAwareMember::new(tag.to_vec(), near, core::WIFI_AWARE_BITRATE_GUESS_BPS),
            far,
        )
    }

    #[test]
    fn the_member_id_is_a_wifi_aware_peer_kind_from_the_tag() {
        let (member, _far) = duplex_member(b"[fe80::1%7]:42720");
        assert_eq!(member.id().kind(), Some(InterfaceKind::WifiAwarePeer));
        let (same, _far) = duplex_member(b"[fe80::1%7]:42720");
        assert_eq!(member.id(), same.id());
        let (other, _far) = duplex_member(b"[fe80::1%9]:42720");
        assert_ne!(member.id(), other.id());
    }

    #[test]
    fn the_member_descriptor_carries_the_declared_bitrate() {
        let (member, _far) = duplex_member(b"peer");
        let descriptor = member.descriptor();
        assert_eq!(descriptor.id, member.id());
        assert_eq!(descriptor.bitrate, core::WIFI_AWARE_BITRATE_GUESS_BPS);
    }
}
