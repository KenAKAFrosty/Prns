use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use prns_core::interfaces::kiss::{KissTransmissionControl, StationIdentification, Transmission};
use prns_core::interfaces::rnode::multi::live::{
    HardwareError, LiveCommand, LiveError, LiveProtocol,
};
use prns_core::interfaces::rnode::{multi, protocol};
use prns_core::interfaces::ConnectionState;
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::throughput::ThroughputLedger;
use prns_runtime::runtime::{AttachedInterface, PrnsNodeHandle};

use crate::byte_stream::deadline::{elapsed_millis, wait_for_deadline};

use super::member::{InboundFrame, LiveMember, MemberMeters, OutboundFrame, RNodeMultiMember};
use super::{RNodeMultiAccess, RNodeMultiMemberSettings};

pub(super) struct RuntimeCycle {
    wire: WireCycle,
    attachments: Vec<AttachedInterface>,
}

pub(super) struct WireCycle {
    pub(super) members: Vec<LiveMember>,
    pub(super) outbound: mpsc::UnboundedReceiver<OutboundFrame>,
    pub(super) live: LiveProtocol,
    pub(super) started: tokio::time::Instant,
}

impl RuntimeCycle {
    pub(super) fn attach<'a>(
        handle: &PrnsNodeHandle,
        settings: impl Iterator<Item = &'a RNodeMultiMemberSettings>,
        station_identification: Option<StationIdentification>,
    ) -> Self {
        let (outbound_tx, outbound) = mpsc::unbounded_channel();
        let started = tokio::time::Instant::now();
        let mut members = Vec::new();
        let mut attachments = Vec::new();
        for settings in settings {
            let (inbound, inbound_rx) = mpsc::unbounded_channel();
            let id = settings.id();
            let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Initializing);
            let member = RNodeMultiMember {
                id,
                vport: settings.vport,
                policy: settings.policy,
                channel_tag: settings.channel_tag.clone(),
                inbound: inbound_rx,
                outbound: outbound_tx.clone(),
                status: status.clone(),
            };
            let attached = match &settings.access {
                RNodeMultiAccess::Open => handle.add_interface(member),
                RNodeMultiAccess::Ifac {
                    context,
                    network_name,
                } => handle.add_interface_with_ifac_name(
                    member,
                    context.as_ref().clone(),
                    network_name.clone(),
                ),
            };
            let _ = handle.set_interface_name(id, settings.name.clone());
            members.push(LiveMember {
                vport: settings.vport,
                radio: settings.radio,
                inbound,
                control: KissTransmissionControl::new(
                    settings.flow_control,
                    station_identification.clone(),
                ),
                meters: MemberMeters {
                    status,
                    airtime: AirtimeLedger::new(),
                    throughput: ThroughputLedger::new(),
                    started,
                    bitrate: settings.policy.bitrate,
                },
            });
            attachments.push(attached);
        }
        let live = LiveProtocol::new(
            members.iter().map(|member| multi::ConfiguredRadio {
                vport: member.vport,
                radio: member.radio,
            }),
            None,
        );
        Self {
            wire: WireCycle {
                members,
                outbound,
                live,
                started,
            },
            attachments,
        }
    }

    pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        decoder: &mut protocol::CommandDecoder,
        read: &mut [u8],
    ) -> io::Result<()> {
        self.wire.serve(stream, decoder, read).await
    }

    pub(super) fn mark_connected(&mut self, platform: Option<multi::DevicePlatform>) {
        self.wire.live.set_platform(platform);
        for member in &self.wire.members {
            member
                .meters
                .status
                .set_connection(ConnectionState::Connected);
        }
    }

    fn teardown(&mut self) {
        for member in &self.wire.members {
            member
                .meters
                .status
                .set_connection(ConnectionState::Disconnected);
        }
        for attached in self.attachments.drain(..) {
            attached.teardown();
        }
    }
}

impl Drop for RuntimeCycle {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl WireCycle {
    async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        decoder: &mut protocol::CommandDecoder,
        read: &mut [u8],
    ) -> io::Result<()> {
        decoder.reset();
        for member in &mut self.members {
            member.control.connection_opened();
        }
        loop {
            let flow_deadline = self
                .members
                .iter()
                .filter_map(|member| member.control.flow_timeout_deadline())
                .min();
            let station_deadline = self
                .members
                .iter()
                .filter_map(|member| member.control.station_identification_deadline())
                .min();
            tokio::select! {
                read_result = stream.read(read) => {
                    let read_count = read_result?;
                    if read_count == 0 {
                        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                    }
                    self.apply_read(&read[..read_count], decoder, stream).await?;
                }
                outbound = self.outbound.recv() => {
                    let Some(outbound) = outbound else {
                        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "all RNodeMulti members stopped"));
                    };
                    self.accept_outbound(outbound, stream).await?;
                }
                () = wait_for_deadline(self.started, flow_deadline) => {
                    self.release_flow_timeouts(stream).await?;
                }
                () = wait_for_deadline(self.started, station_deadline) => {
                    self.emit_station_identification(stream).await?;
                }
            }
        }
    }

    pub(super) async fn apply_read<S: AsyncWrite + Unpin>(
        &mut self,
        bytes: &[u8],
        decoder: &mut protocol::CommandDecoder,
        stream: &mut S,
    ) -> io::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let Some((command, payload)) =
                decoder.feed_slice_next(bytes, &mut offset).ok().flatten()
            else {
                continue;
            };
            match self.live.apply(command, payload) {
                LiveCommand::Data {
                    vport,
                    payload,
                    phy,
                } => self.deliver_inbound(vport, payload, phy),
                LiveCommand::AllRadiosReady => self.release_ready(stream).await?,
                LiveCommand::Consumed => {}
                LiveCommand::Failed(error) => return Err(live_error(error)),
            }
        }
        Ok(())
    }

    fn deliver_inbound(
        &mut self,
        vport: multi::VPort,
        payload: &[u8],
        phy: prns_core::interfaces::PacketPhyStats,
    ) {
        let Some(member) = self.member_mut(vport) else {
            return;
        };
        let wire_bytes = multi::data_frame(vport, payload)
            .map(|frame| frame.len())
            .unwrap_or(payload.len());
        member.meters.record_rx(wire_bytes);
        let _ = member.inbound.send(InboundFrame {
            payload: payload.to_vec(),
            phy,
        });
    }

    pub(super) async fn accept_outbound<S: AsyncWrite + Unpin>(
        &mut self,
        outbound: OutboundFrame,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = elapsed_millis(self.started);
        let Some(index) = self.member_index(outbound.vport) else {
            return Ok(());
        };
        if let Some(transmission) = self.members[index]
            .control
            .accept_packet(&outbound.payload, now)
        {
            self.write_transmission(index, transmission, stream).await?;
        }
        Ok(())
    }

    pub(super) async fn release_ready<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = elapsed_millis(self.started);
        for index in 0..self.members.len() {
            if let Some(transmission) = self.members[index].control.ready_received(now) {
                self.write_transmission(index, transmission, stream).await?;
            }
        }
        Ok(())
    }

    async fn release_flow_timeouts<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = elapsed_millis(self.started);
        for index in 0..self.members.len() {
            if let Some(transmission) = self.members[index].control.flow_timeout_elapsed(now) {
                self.write_transmission(index, transmission, stream).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn emit_station_identification<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = elapsed_millis(self.started);
        for index in 0..self.members.len() {
            if let Some(transmission) = self.members[index]
                .control
                .station_identification_elapsed(now)
            {
                self.write_transmission(index, transmission, stream).await?;
            }
        }
        Ok(())
    }

    async fn write_transmission<S: AsyncWrite + Unpin>(
        &mut self,
        index: usize,
        transmission: Transmission,
        stream: &mut S,
    ) -> io::Result<()> {
        let is_packet = transmission.is_packet();
        let frame =
            multi::data_frame(self.members[index].vport, transmission.payload()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "RNodeMulti frame exceeds hardware MTU",
                )
            })?;
        stream.write_all(&frame).await?;
        let now = elapsed_millis(self.started);
        self.members[index].control.transmitted(&transmission, now);
        self.members[index].meters.record_tx(frame.len());
        if is_packet {
            for member in &mut self.members {
                member.control.arm_station_identification(now);
            }
        }
        Ok(())
    }

    fn member_index(&self, vport: multi::VPort) -> Option<usize> {
        self.members.iter().position(|member| member.vport == vport)
    }

    fn member_mut(&mut self, vport: multi::VPort) -> Option<&mut LiveMember> {
        let index = self.member_index(vport)?;
        self.members.get_mut(index)
    }
}

fn live_error(error: LiveError) -> io::Error {
    let (kind, message) = match error {
        LiveError::Hardware(HardwareError::RadioInitialization) => (
            io::ErrorKind::Other,
            "RNodeMulti radio initialisation failure",
        ),
        LiveError::Hardware(HardwareError::Transmit) => {
            (io::ErrorKind::Other, "RNodeMulti hardware transmit failure")
        }
        LiveError::Hardware(HardwareError::EepromLocked) => {
            (io::ErrorKind::Other, "RNodeMulti EEPROM is locked")
        }
        LiveError::Hardware(HardwareError::Unknown(_)) => {
            (io::ErrorKind::Other, "RNodeMulti unknown hardware failure")
        }
        LiveError::Esp32Reset => (
            io::ErrorKind::ConnectionReset,
            "RNodeMulti ESP32 reset while online",
        ),
    };
    io::Error::new(kind, message)
}
