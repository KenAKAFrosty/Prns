use core::convert::Infallible;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Duration;
use prns_core::entropy::EntropySource;

use crate::runtime::{EntropyHandle, SharedRuntimeEntropy};

use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, TransportCapability,
};

pub(crate) const WATCHDOG: Duration = Duration::from_secs(5);

pub(crate) struct TestEntropySource;

impl EntropySource for TestEntropySource {
    type Error = Infallible;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        output.fill(0x41);
        Ok(())
    }
}

pub(crate) fn entropy_handle() -> EntropyHandle<CriticalSectionRawMutex, TestEntropySource> {
    Box::leak(Box::new(
        SharedRuntimeEntropy::try_new(TestEntropySource)
            .expect("the deterministic test source always seeds"),
    ))
    .handle()
}

pub(crate) fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: crate::interfaces::InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
    }
}
