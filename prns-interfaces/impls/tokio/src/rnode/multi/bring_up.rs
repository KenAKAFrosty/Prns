use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use prns_core::interfaces::rnode::multi::bring_up::{
    BringUp, BringUpAction, BringUpError, ConfiguredRadio,
};
use prns_core::interfaces::rnode::multi::DevicePlatform;
use prns_core::interfaces::rnode::protocol;

use crate::byte_stream::deadline::{elapsed_millis, instant_for};

use super::{RNodeMultiConfigureDelay, RNodeMultiMemberSettings};

pub(super) async fn bring_up<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    members: &[RNodeMultiMemberSettings],
    configure_delay: RNodeMultiConfigureDelay,
    decoder: &mut protocol::CommandDecoder,
    read: &mut [u8],
) -> io::Result<Option<DevicePlatform>> {
    decoder.reset();
    let started = tokio::time::Instant::now();
    let radios = members
        .iter()
        .map(|member| ConfiguredRadio {
            vport: member.vport,
            radio: member.radio,
        })
        .collect();
    let mut protocol = BringUp::new(radios, configure_delay);
    loop {
        match protocol.next_action(elapsed_millis(started)) {
            BringUpAction::WriteDetect(bytes) => stream.write_all(&bytes).await?,
            BringUpAction::WriteRadioConfiguration { bytes, .. } => {
                stream.write_all(&bytes).await?;
            }
            BringUpAction::SleepUntil(deadline) => {
                let Some(deadline) = instant_for(started, deadline) else {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "RNodeMulti configure delay exceeds the host clock range",
                    ));
                };
                tokio::time::sleep_until(deadline).await;
                protocol.deadline_elapsed(elapsed_millis(started));
            }
            BringUpAction::ReadUntil(deadline) => {
                let Some(deadline) = instant_for(started, deadline) else {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "RNodeMulti bring-up deadline exceeds the host clock range",
                    ));
                };
                match tokio::time::timeout_at(deadline, stream.read(read)).await {
                    Err(_) => protocol.deadline_elapsed(elapsed_millis(started)),
                    Ok(Ok(0)) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                    Ok(Ok(read_count)) => {
                        let now = elapsed_millis(started);
                        decoder.feed_slice(&read[..read_count], |command, payload| {
                            protocol.apply_command(command, payload, now);
                        });
                    }
                    Ok(Err(error)) => return Err(error),
                }
            }
            BringUpAction::Complete(platform) => return Ok(platform),
            BringUpAction::Failed(error) => return Err(bring_up_error(error)),
        }
    }
}

fn bring_up_error(error: BringUpError) -> io::Error {
    match error {
        BringUpError::DetectTimedOut => io::Error::new(
            io::ErrorKind::TimedOut,
            "RNodeMulti device did not answer the detect query",
        ),
        BringUpError::MissingInterfaceInventory => io::Error::new(
            io::ErrorKind::TimedOut,
            "RNodeMulti device did not report its radio inventory",
        ),
        BringUpError::MissingFirmwareVersion => io::Error::new(
            io::ErrorKind::TimedOut,
            "RNodeMulti device did not report its firmware version",
        ),
        BringUpError::FirmwareTooOld { reported, required } => io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "RNodeMulti firmware {}.{} is too old; version {}.{} or newer is required",
                reported.major, reported.minor, required.major, required.minor
            ),
        ),
        BringUpError::MissingVPort {
            vport,
            reported_radio_count,
        } => io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "RNodeMulti vport {} is not present; the device reported {} radio(s)",
                vport.get(),
                reported_radio_count
            ),
        ),
        BringUpError::UnsupportedFrequency {
            vport,
            radio_type,
            frequency,
        } => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "RNodeMulti vport {} reports {radio_type:?}, which does not support {} Hz",
                vport.get(),
                frequency.hz()
            ),
        ),
        BringUpError::RadioMismatch { vport } => io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "RNodeMulti vport {} reported radio parameters that do not match its configuration",
                vport.get()
            ),
        ),
    }
}
