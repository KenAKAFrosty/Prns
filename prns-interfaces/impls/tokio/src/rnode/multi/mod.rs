use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use prns_core::interfaces::kiss::{ReadyCommandFlowControl, StationIdentification};
use prns_core::interfaces::rnode::{multi, protocol};
use prns_core::interfaces::IfacContext;
use prns_core::interfaces::{EffectiveInterfacePolicy, InterfaceId, InterfaceKind};
use prns_runtime::runtime::PrnsNodeHandle;

use crate::reconnect::ReconnectPolicy;
use crate::rnode::RNodeResetDelay;

mod bring_up;
mod member;
mod wire;

use bring_up::bring_up;
use wire::RuntimeCycle;

pub use prns_core::interfaces::rnode::multi::bring_up::ConfigureDelay as RNodeMultiConfigureDelay;

pub const DEFAULT_RNODE_MULTI_CONFIGURE_DELAY: RNodeMultiConfigureDelay =
    prns_core::interfaces::rnode::multi::bring_up::DEFAULT_CONFIGURE_DELAY;

#[derive(Clone)]
pub enum RNodeMultiAccess {
    Open,
    Ifac {
        context: Box<IfacContext>,
        network_name: Option<String>,
    },
}

#[derive(Clone)]
pub struct RNodeMultiMemberSettings {
    name: String,
    vport: multi::VPort,
    radio: multi::RadioConfig,
    flow_control: ReadyCommandFlowControl,
    policy: EffectiveInterfacePolicy,
    access: RNodeMultiAccess,
    channel_tag: Vec<u8>,
}

impl RNodeMultiMemberSettings {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        vport: multi::VPort,
        radio: multi::RadioConfig,
        flow_control: ReadyCommandFlowControl,
        policy: EffectiveInterfacePolicy,
        access: RNodeMultiAccess,
        parent_channel_tag: &[u8],
    ) -> Self {
        let mut channel_tag = parent_channel_tag.to_vec();
        channel_tag.push(0);
        channel_tag.push(vport.get());
        Self {
            name: name.into(),
            vport,
            radio,
            flow_control,
            policy,
            access,
            channel_tag,
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::Rnode, &self.channel_tag)
    }

    #[must_use]
    pub const fn vport(&self) -> multi::VPort {
        self.vport
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RNodeMultiMembersError {
    Empty,
    DuplicateVPort(multi::VPort),
}

impl std::fmt::Display for RNodeMultiMembersError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("RNodeMulti requires at least one member"),
            Self::DuplicateVPort(vport) => {
                write!(
                    formatter,
                    "RNodeMulti vport {} is configured twice",
                    vport.get()
                )
            }
        }
    }
}

impl std::error::Error for RNodeMultiMembersError {}

pub struct RNodeMultiMembers(Vec<RNodeMultiMemberSettings>);

impl RNodeMultiMembers {
    pub fn new(members: Vec<RNodeMultiMemberSettings>) -> Result<Self, RNodeMultiMembersError> {
        if members.is_empty() {
            return Err(RNodeMultiMembersError::Empty);
        }
        let mut vports = HashSet::with_capacity(members.len());
        for member in &members {
            if !vports.insert(member.vport) {
                return Err(RNodeMultiMembersError::DuplicateVPort(member.vport));
            }
        }
        Ok(Self(members))
    }

    pub fn iter(&self) -> impl Iterator<Item = &RNodeMultiMemberSettings> {
        self.0.iter()
    }

    #[must_use]
    pub fn as_slice(&self) -> &[RNodeMultiMemberSettings] {
        &self.0
    }
}

pub struct RNodeMultiInterface<Open> {
    name: String,
    device: String,
    open: Open,
    reconnect_policy: ReconnectPolicy,
    reset_delay: RNodeResetDelay,
    configure_delay: RNodeMultiConfigureDelay,
    station_identification: Option<StationIdentification>,
    members: RNodeMultiMembers,
}

pub struct RNodeMultiSettings {
    pub reconnect_policy: ReconnectPolicy,
    pub reset_delay: RNodeResetDelay,
    pub configure_delay: RNodeMultiConfigureDelay,
    pub station_identification: Option<StationIdentification>,
    pub members: RNodeMultiMembers,
}

pub struct RegisteredRNodeMultiInterface<Open> {
    interface: RNodeMultiInterface<Open>,
    handle: PrnsNodeHandle,
    cycle: RuntimeCycle,
}

impl<Open> RNodeMultiInterface<Open> {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        device: impl Into<String>,
        open: Open,
        settings: RNodeMultiSettings,
    ) -> Self {
        Self {
            name: name.into(),
            device: device.into(),
            open,
            reconnect_policy: settings.reconnect_policy,
            reset_delay: settings.reset_delay,
            configure_delay: settings.configure_delay,
            station_identification: settings.station_identification,
            members: settings.members,
        }
    }

    pub fn member_ids(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.members.iter().map(RNodeMultiMemberSettings::id)
    }

    #[must_use]
    pub fn register(self, handle: &PrnsNodeHandle) -> RegisteredRNodeMultiInterface<Open> {
        let cycle = RuntimeCycle::attach(
            handle,
            self.members.iter(),
            self.station_identification.clone(),
        );
        RegisteredRNodeMultiInterface {
            interface: self,
            handle: handle.clone(),
            cycle,
        }
    }
}

impl<Open, Fut, S> RegisteredRNodeMultiInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn run(mut self) {
        let mut decoder = Box::new(protocol::CommandDecoder::new());
        let mut read = vec![0u8; protocol::READ_BUF_LEN].into_boxed_slice();
        let mut cycle = self.cycle;
        let mut reconnect = self.interface.reconnect_policy.schedule();
        loop {
            let mut connected_at = None;
            let result = self
                .interface
                .run_connection(&mut cycle, &mut decoder, &mut read, &mut connected_at)
                .await;
            drop(cycle);
            if let Some(connected_at) = connected_at {
                reconnect.record_connection_lifetime(connected_at.elapsed());
            }
            let reconnect_delay = reconnect.next_delay(|bytes| self.handle.fill_entropy(bytes));
            if let Err(error) = result {
                report_connection_failure(
                    &self.interface.name,
                    &self.interface.device,
                    reconnect_delay,
                    &error,
                );
            }
            tokio::time::sleep(reconnect_delay).await;
            cycle = RuntimeCycle::attach(
                &self.handle,
                self.interface.members.iter(),
                self.interface.station_identification.clone(),
            );
        }
    }
}

impl<Open> RNodeMultiInterface<Open> {
    async fn run_connection<Fut, S>(
        &mut self,
        cycle: &mut RuntimeCycle,
        decoder: &mut protocol::CommandDecoder,
        read: &mut [u8],
        connected_at: &mut Option<tokio::time::Instant>,
    ) -> io::Result<()>
    where
        Open: FnMut() -> Fut,
        Fut: Future<Output = io::Result<S>>,
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut stream = (self.open)().await?;
        if !self.reset_delay.duration().is_zero() {
            tokio::time::sleep(self.reset_delay.duration()).await;
        }
        let platform = bring_up(
            &mut stream,
            self.members.as_slice(),
            self.configure_delay,
            decoder,
            read,
        )
        .await?;
        cycle.mark_connected(platform);
        *connected_at = Some(tokio::time::Instant::now());
        cycle.serve(&mut stream, decoder, read).await
    }
}

fn report_connection_failure(
    name: &str,
    device: &str,
    reconnect_delay: Duration,
    error: &io::Error,
) {
    #[cfg(feature = "tracing")]
    tracing::warn!(
        target: "prns.interface",
        event = "rnode_multi_connection_failed",
        interface_name = name,
        device,
        retry_after_ms = reconnect_delay.as_millis() as u64,
        error = %error,
    );
    #[cfg(not(feature = "tracing"))]
    crate::diagnostic_log::error!(
        "RNodeMulti interface [{name}] on {device}: {error}; retrying in {:?}",
        reconnect_delay
    );
}

#[cfg(test)]
mod tests;
