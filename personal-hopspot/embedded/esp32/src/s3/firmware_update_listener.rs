use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use personal_rns::identity::IdentityHash;
use personal_rns::routing::links::channel::byte_stream;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent};
use personal_rns::wire::DestinationHash;

use crate::firmware_update::{self, InstallError, InstallGuard};
use crate::memory::EspFirmwareMemory;

const SCRATCH: usize = 1024;
const QUEUE: usize = 4;

static DESTINATION: Mutex<CriticalSectionRawMutex, Cell<Option<DestinationHash>>> =
    Mutex::new(Cell::new(None));
static EVENTS: Channel<CriticalSectionRawMutex, OtaEvent, QUEUE> = Channel::new();

enum OtaEvent {
    Link {
        link_id: LinkId,
        destination: DestinationHash,
    },
    Identified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    Closed {
        link_id: LinkId,
    },
    Chunk {
        link_id: LinkId,
        bytes: heapless::Vec<u8, SCRATCH>,
        eof: bool,
    },
}

pub(super) fn set_destination(destination: DestinationHash) {
    log::info!("update: listening on {}", hex16(destination.as_bytes()));
    DESTINATION.lock(|slot| slot.set(Some(destination)));
}

fn hex16(bytes: &[u8; 16]) -> heapless::String<32> {
    const NIBBLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = heapless::String::new();
    for byte in bytes {
        let _ = out.push(NIBBLE[(byte >> 4) as usize] as char);
        let _ = out.push(NIBBLE[(byte & 0x0f) as usize] as char);
    }
    out
}

fn permitted(identity: IdentityHash) -> bool {
    personal_rns::runtime::firmware_update_permitted(&identity)
}

fn log_permit(identity: IdentityHash) {
    let noted = personal_rns::runtime::firmware_update_noted_grant_count();
    let allowed = permitted(identity);
    log::info!(
        "update: permit peer={} noted={noted} permitted={allowed}",
        hex16(identity.as_bytes())
    );
    let mut index = 0u8;
    while index < noted {
        if let Some(grant) = personal_rns::runtime::firmware_update_noted_grant(index) {
            log::info!("update: permit grant {index} {}", hex16(grant.as_bytes()));
        }
        index = index.saturating_add(1);
    }
}

pub(super) fn observe(event: &PrnsEvent<'_>) {
    let queued = match event {
        PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established)) => Some(OtaEvent::Link {
            link_id: established.link_id,
            destination: established.destination,
        }),
        PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { link_id, identity }) => {
            Some(OtaEvent::Identified { link_id, identity })
        }
        PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, .. }) => {
            Some(OtaEvent::Closed { link_id })
        }
        PrnsEvent::Message(Message::ChannelMessage {
            link_id,
            message_type,
            data,
        }) if message_type == byte_stream::STREAM_DATA_TYPE => {
            let Ok(frame) = byte_stream::parse(data) else {
                return;
            };
            if frame.header.compressed || frame.payload.len() > SCRATCH {
                return;
            }
            let mut bytes = heapless::Vec::new();
            if bytes.extend_from_slice(frame.payload).is_err() {
                return;
            }
            Some(OtaEvent::Chunk {
                link_id,
                bytes,
                eof: frame.header.eof,
            })
        }
        _ => None,
    };
    if let Some(event) = queued {
        let _ = EVENTS.try_send(event);
    }
}

#[embassy_executor::task]
pub(super) async fn firmware_update_listener_task(
    handle: crate::s3::Handle,
    profile: &'static personal_hopspot_memory::MemoryProfile,
) {
    let memory = EspFirmwareMemory::new(profile);
    let mut link: Option<LinkId> = None;
    let mut peer: Option<IdentityHash> = None;
    let mut install: Option<firmware_update::FirmwareInstall> = None;
    let mut digest_staged = false;
    loop {
        match EVENTS.receive().await {
            OtaEvent::Link {
                link_id,
                destination,
            } => {
                let wanted = DESTINATION.lock(|slot| slot.get());
                let matched = wanted == Some(destination);
                log::info!(
                    "update: link established match={matched} dest={}",
                    hex16(destination.as_bytes())
                );
                if matched && link.is_none() {
                    link = Some(link_id);
                    peer = None;
                    digest_staged = false;
                }
            }
            OtaEvent::Identified { link_id, identity } => {
                if link == Some(link_id) {
                    log_permit(identity);
                    peer = Some(identity);
                }
            }
            OtaEvent::Closed { link_id } => {
                if link == Some(link_id) {
                    log::info!("update: link closed");
                    link = None;
                    peer = None;
                    install = None;
                    digest_staged = false;
                }
            }
            OtaEvent::Chunk {
                link_id,
                bytes,
                eof,
            } => {
                if link != Some(link_id) {
                    continue;
                }
                let Some(identity) = peer else {
                    continue;
                };
                if !permitted(identity) {
                    continue;
                }
                if let Err(error) = feed(
                    &handle,
                    &memory,
                    link_id,
                    &mut install,
                    &mut digest_staged,
                    &bytes,
                    eof,
                )
                .await
                {
                    log::warn!("update: stream dropped: {error:?}");
                    install = None;
                    digest_staged = false;
                    link = None;
                }
            }
        }
    }
}

async fn feed(
    handle: &crate::s3::Handle,
    memory: &EspFirmwareMemory,
    link_id: LinkId,
    install: &mut Option<firmware_update::FirmwareInstall>,
    digest_staged: &mut bool,
    bytes: &[u8],
    eof: bool,
) -> Result<(), InstallError> {
    if !*digest_staged {
        let digest: [u8; 32] = bytes
            .try_into()
            .map_err(|_| InstallError::DigestMalformed)?;
        firmware_update::stage_digest(digest)?;
        *digest_staged = true;
        log::info!("update: digest staged");
        return Ok(());
    }
    if install.is_none() {
        if bytes.len() != 4 {
            return Err(InstallError::ImageTooSmall { image_len: 0 });
        }
        let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let guard = InstallGuard::acquire()?;
        *install = Some(firmware_update::begin(memory, guard, declared)?);
        return Ok(());
    }
    if !bytes.is_empty() {
        install
            .as_mut()
            .expect("the session was just confirmed")
            .write(bytes)
            .await?;
    }
    if eof {
        let finished = install
            .take()
            .expect("the session is present")
            .finish()
            .await?;
        let mut reply = heapless::Vec::<u8, 64>::new();
        let _ = reply.extend_from_slice(b"ok slot=");
        let _ = reply.extend_from_slice(firmware_update::slot_name(finished.slot).as_bytes());
        let _ = reply.extend_from_slice(b" bytes=");
        push_usize(&mut reply, finished.image_len);
        let _ = reply.extend_from_slice(b"\n");
        let _ = handle
            .send_channel_message(link_id, MessageType(0xff00), &reply)
            .await;
        Timer::after(Duration::from_millis(500)).await;
        core::mem::forget(finished);
        esp_hal::system::software_reset();
    }
    Ok(())
}

fn push_usize(buf: &mut heapless::Vec<u8, 64>, mut value: usize) {
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    if value == 0 {
        let _ = buf.push(b'0');
        return;
    }
    while value > 0 {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
    }
    while count > 0 {
        count -= 1;
        let _ = buf.push(digits[count]);
    }
}
