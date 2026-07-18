use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use prns_core::interfaces::rnode::{core, multi};
use prns_core::interfaces::ConnectionState;
use prns_runtime::reactor::airtime::AirtimeLedger;
use prns_runtime::reactor::driver::TokioInterfaceStatus;
use prns_runtime::reactor::throughput::ThroughputLedger;
use prns_runtime::runtime::{AttachedInterface, PrnsNodeHandle};

use crate::serial_control::{
    wait_for_deadline, SerialControl, StationIdentification, Transmission,
};

use super::member::{InboundFrame, LiveMember, MemberMeters, OutboundFrame, RNodeMultiMember};
use super::{RNodeMultiAccess, RNodeMultiMemberSettings};

pub(super) struct RuntimeCycle {
    wire: WireCycle,
    attachments: Vec<AttachedInterface>,
}

pub(super) struct WireCycle {
    pub(super) members: Vec<LiveMember>,
    pub(super) outbound: mpsc::UnboundedReceiver<OutboundFrame>,
    pub(super) selected: multi::VPort,
    pub(super) platform: Option<multi::DevicePlatform>,
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
            let status = TokioInterfaceStatus::new(id, ConnectionState::Initializing);
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
                control: SerialControl::new(settings.flow_control, station_identification.clone()),
                packet_phy: multi::PacketPhyState::default(),
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
        Self {
            wire: WireCycle {
                members,
                outbound,
                selected: multi::VPort::ZERO,
                platform: None,
            },
            attachments,
        }
    }

    pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        decoder: &mut core::CommandDecoder,
        read: &mut [u8],
    ) -> io::Result<()> {
        self.wire.serve(stream, decoder, read).await
    }

    pub(super) fn mark_connected(&mut self, platform: Option<multi::DevicePlatform>) {
        self.wire.platform = platform;
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
        decoder: &mut core::CommandDecoder,
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
                () = wait_for_deadline(flow_deadline) => {
                    self.release_flow_timeouts(stream).await?;
                }
                () = wait_for_deadline(station_deadline) => {
                    self.emit_station_identification(stream).await?;
                }
            }
        }
    }

    pub(super) async fn apply_read<S: AsyncWrite + Unpin>(
        &mut self,
        bytes: &[u8],
        decoder: &mut core::CommandDecoder,
        stream: &mut S,
    ) -> io::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let Some((command, payload)) =
                decoder.feed_slice_next(bytes, &mut offset).ok().flatten()
            else {
                continue;
            };
            match command {
                multi::CMD_SELECT_INTERFACE => {
                    if let Some(vport) = payload.first().and_then(|value| multi::VPort::new(*value))
                    {
                        self.selected = vport;
                    }
                }
                core::CMD_DATA => self.deliver_inbound(payload),
                prns_core::interfaces::kiss_framing::CMD_READY => {
                    self.release_ready(stream).await?;
                }
                core::CMD_PLATFORM => {
                    self.platform = payload
                        .first()
                        .copied()
                        .map(multi::DevicePlatform::from_device_report);
                }
                core::CMD_ERROR => return Err(hardware_error(payload)),
                core::CMD_RESET
                    if self.platform == Some(multi::DevicePlatform::Esp32)
                        && payload.first() == Some(&core::RESET_RESP) =>
                {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "RNodeMulti ESP32 reset while online",
                    ));
                }
                _ => {
                    if let Some(member) = self.member_mut(self.selected) {
                        member.packet_phy.apply(command, payload, member.radio);
                    }
                }
            }
        }
        Ok(())
    }

    fn deliver_inbound(&mut self, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let selected = self.selected;
        let Some(member) = self.member_mut(selected) else {
            return;
        };
        let phy = member.packet_phy.take_for_data();
        let wire_len = multi::data_frame(selected, payload)
            .map(|frame| frame.len())
            .unwrap_or(payload.len());
        member.meters.record_rx(wire_len);
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
        let now = tokio::time::Instant::now();
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
        let now = tokio::time::Instant::now();
        for index in 0..self.members.len() {
            if let Some(transmission) = self.members[index].control.ready(now) {
                self.write_transmission(index, transmission, stream).await?;
            }
        }
        Ok(())
    }

    async fn release_flow_timeouts<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = tokio::time::Instant::now();
        for index in 0..self.members.len() {
            let due = self.members[index]
                .control
                .flow_timeout_deadline()
                .is_some_and(|deadline| deadline <= now);
            if due {
                if let Some(transmission) = self.members[index].control.ready(now) {
                    self.write_transmission(index, transmission, stream).await?;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn emit_station_identification<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<()> {
        let now = tokio::time::Instant::now();
        for index in 0..self.members.len() {
            let due = self.members[index]
                .control
                .station_identification_deadline()
                .is_some_and(|deadline| deadline <= now);
            if due {
                if let Some(transmission) =
                    self.members[index].control.station_identification_due(now)
                {
                    self.write_transmission(index, transmission, stream).await?;
                }
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
        let now = tokio::time::Instant::now();
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

fn hardware_error(payload: &[u8]) -> io::Error {
    let message = match payload.first().copied() {
        Some(core::ERROR_INIT_RADIO) => "RNodeMulti radio initialisation failure",
        Some(core::ERROR_TX_FAILED) => "RNodeMulti hardware transmit failure",
        Some(core::ERROR_EEPROM_LOCKED) => "RNodeMulti EEPROM is locked",
        _ => "RNodeMulti unknown hardware failure",
    };
    io::Error::other(message)
}
