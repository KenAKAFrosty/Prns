use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use prns_core::interfaces::i2p::core;
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, FrameSink, InterfaceId, InterfaceStatus,
};
use prns_runtime::reactor::airtime::AirtimeLedger;
use prns_runtime::reactor::driver::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::InterfaceSeam;
use prns_runtime::reactor::throughput::ThroughputLedger;

use crate::framed_stream::{serve_with_hdlc_idle_watchdog, FramedBuffers, HdlcFraming, WireMeters};

struct PendingSeam {
    inbound: Vec<u8>,
}

impl InterfaceSeam for PendingSeam {
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.inbound
    }

    async fn commit_inbound(&mut self) {
        self.inbound.clear();
    }

    async fn next_outbound(&mut self) -> &[u8] {
        std::future::pending::<()>().await;
        &[]
    }
}

#[tokio::test(start_paused = true)]
async fn watchdog_keepalive_degradation_recovery_and_timeout_match_reference_timing() {
    let status = TokioInterfaceStatus::new(InterfaceId::new([0x71; 8]), ConnectionState::Connected);
    let task_status = status.clone();
    let (local, mut remote) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
        let mut buffers = FramedBuffers::<HdlcFraming, 128, 256>::new();
        let mut seam = PendingSeam {
            inbound: Vec::new(),
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        serve_with_hdlc_idle_watchdog(
            local,
            &mut buffers,
            &mut seam,
            &mut WireMeters {
                status: &task_status,
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: policy.bitrate,
                started,
            },
        )
        .await;
    });

    tokio::time::advance(Duration::from_secs(11)).await;
    let mut keepalive = [0u8; 2];
    remote
        .read_exact(&mut keepalive)
        .await
        .expect("the reference keepalive is written");
    assert_eq!(keepalive, [0x7e, 0x7e]);

    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(status.connection(), ConnectionState::Degraded);

    remote
        .write_all(&[0x7e, 0x7e])
        .await
        .expect("inbound activity reaches the interface");
    tokio::task::yield_now().await;
    assert_eq!(status.connection(), ConnectionState::Connected);

    tokio::time::advance(Duration::from_secs(111)).await;
    task.await
        .expect("the watchdog exits after its read timeout");
}
