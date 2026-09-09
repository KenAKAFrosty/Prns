//! Queue-confined admission for one CoreBluetooth ATT write callback.
//!
//! Validate and reserve the entire batch before publishing any session or message. After the
//! admission boundary, receiver teardown is delivery loss, not a fallible partial batch commit.

use std::collections::HashMap;

use tokio::sync::mpsc;

use prns_core::interfaces::bluetooth_auto::{BleIdentity, Control, PeerProtocol};

use super::gatt_link::{
    gatt_inbound_channel, GattInboundReceiver, GattInboundReservation, GattInboundSender,
};
use super::peripheral::can_open_inbound;
use super::CoreBluetoothPeerId;

#[derive(Clone, Copy)]
pub(super) enum InboundProfile {
    Native,
    Columba(BleIdentity),
}

impl InboundProfile {
    pub(super) fn protocol(self) -> PeerProtocol {
        match self {
            Self::Native => PeerProtocol::Native,
            Self::Columba(_) => PeerProtocol::Columba,
        }
    }

    pub(super) fn peer_identity(self) -> Option<BleIdentity> {
        match self {
            Self::Native => None,
            Self::Columba(identity) => Some(identity),
        }
    }
}

// The context is the retained CBCentral in production. Keeping only that platform object
// generic lets tests exercise the actual admission code without constructing Bluetooth objects.
pub(super) struct WriteSession<C> {
    pub(super) central: C,
    pub(super) protocol: PeerProtocol,
    pub(super) control_tx: mpsc::Sender<Control>,
    pub(super) data_tx: GattInboundSender,
}

#[derive(Clone, Copy)]
pub(super) enum WriteTarget {
    Control,
    Data,
    ColumbaRx,
    Unsupported,
}

pub(super) struct WriteRequest<C> {
    pub(super) central: C,
    pub(super) peer_id: CoreBluetoothPeerId,
    pub(super) target: WriteTarget,
    pub(super) offset: usize,
    pub(super) value: Option<Box<[u8]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteError {
    InsufficientResources,
    InvalidOffset,
    InvalidValueLength,
    WriteNotPermitted,
}

enum ReservedWrite {
    Control(mpsc::OwnedPermit<Control>, Control),
    Data(GattInboundReservation),
}

impl ReservedWrite {
    fn commit(self) {
        match self {
            Self::Control(permit, control) => {
                permit.send(control);
            }
            Self::Data(reservation) => reservation.commit(),
        }
    }
}

/// CoreBluetooth supplies at least one request. An empty defensive input has no request to
/// answer; every nonempty callback executes admission once and answers only its first request.
pub(super) fn respond_to_write_batch<R>(
    first: Option<&R>,
    admit: impl FnOnce() -> Result<(), WriteError>,
    respond: impl FnOnce(&R, Result<(), WriteError>),
) {
    if let Some(first) = first {
        let result = admit();
        respond(first, result);
    }
}

pub(super) fn admit_write_batch<C: Clone, L>(
    radio_enabled: bool,
    requests: impl IntoIterator<Item = Result<WriteRequest<C>, WriteError>>,
    sessions: &mut HashMap<CoreBluetoothPeerId, WriteSession<C>>,
    session_capacity: usize,
    inbound: &mpsc::Sender<L>,
    mut make_link: impl FnMut(
        &WriteRequest<C>,
        InboundProfile,
        mpsc::Receiver<Control>,
        GattInboundReceiver,
    ) -> L,
) -> Result<(), WriteError> {
    if !radio_enabled {
        return Err(WriteError::InsufficientResources);
    }
    let mut new_sessions = HashMap::<CoreBluetoothPeerId, WriteSession<C>>::new();
    let mut new_links = Vec::new();
    let mut writes = Vec::new();

    for request in requests {
        let mut request = request?;
        // These are message endpoints, not mutable long-write attributes. Never interpret an
        // offset fragment as a complete control message or transport frame.
        if request.offset != 0 {
            return Err(WriteError::InvalidOffset);
        }
        let value = request
            .value
            .as_deref()
            .ok_or(WriteError::InvalidValueLength)?;
        let protocol = new_sessions
            .get(&request.peer_id)
            .or_else(|| sessions.get(&request.peer_id))
            .map(|session| session.protocol);
        let mut control = None;
        let mut data = false;
        let new_profile = match (request.target, protocol) {
            (WriteTarget::Control, None | Some(PeerProtocol::Native)) => {
                control = Some(Control::decode(value).ok_or(WriteError::InvalidValueLength)?);
                protocol.is_none().then_some(InboundProfile::Native)
            }
            (WriteTarget::Data, Some(PeerProtocol::Native))
            | (WriteTarget::ColumbaRx, Some(PeerProtocol::Columba)) => {
                // Preserve existing data-fragment admission, including its charged empty values.
                data = true;
                None
            }
            (WriteTarget::ColumbaRx, None) => {
                let identity: [u8; 16] = value
                    .try_into()
                    .map_err(|_| WriteError::InvalidValueLength)?;
                Some(InboundProfile::Columba(BleIdentity::new(identity)))
            }
            _ => return Err(WriteError::WriteNotPermitted),
        };

        if let Some(profile) = new_profile {
            if !can_open_inbound(
                sessions.len().saturating_add(new_sessions.len()),
                session_capacity,
            ) {
                return Err(WriteError::InsufficientResources);
            }
            let permit = inbound
                .clone()
                .try_reserve_owned()
                .map_err(|_| WriteError::InsufficientResources)?;
            let (control_tx, control_rx) = mpsc::channel(8);
            let (data_tx, data_rx) = gatt_inbound_channel();
            let link = make_link(&request, profile, control_rx, data_rx);
            new_sessions.insert(
                request.peer_id,
                WriteSession {
                    central: request.central.clone(),
                    protocol: profile.protocol(),
                    control_tx,
                    data_tx,
                },
            );
            new_links.push((permit, link));
        }

        let session = new_sessions
            .get(&request.peer_id)
            .or_else(|| sessions.get(&request.peer_id))
            .ok_or(WriteError::WriteNotPermitted)?;
        if let Some(control) = control {
            let permit = session
                .control_tx
                .clone()
                .try_reserve_owned()
                .map_err(|_| WriteError::InsufficientResources)?;
            writes.push(ReservedWrite::Control(permit, control));
        } else if data {
            let reservation = session
                .data_tx
                .try_reserve(request.value.take().ok_or(WriteError::InvalidValueLength)?)
                .map_err(|_| WriteError::InsufficientResources)?;
            writes.push(ReservedWrite::Data(reservation));
        }
    }

    // Admission boundary: every operation below is infallible. Owned permits and data byte
    // reservations roll back on every earlier return. A receiver closing after reservation is
    // ordinary teardown of accepted work and must not turn this into a partially rejected batch.
    sessions.extend(new_sessions);
    for write in writes {
        write.commit();
    }
    // Publish new links last, with their initial input already queued. No staged receiver or
    // session escapes on a refused batch.
    for (permit, link) in new_links {
        permit.send(link);
    }
    Ok(())
}
