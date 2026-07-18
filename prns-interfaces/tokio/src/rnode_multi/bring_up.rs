use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use prns_core::interfaces::rnode::{core, multi};

use super::{RNodeMultiConfigureDelay, RNodeMultiMemberSettings};

const DETECT_TIMEOUT: Duration = Duration::from_secs(2);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) async fn bring_up<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    members: &[RNodeMultiMemberSettings],
    configure_delay: RNodeMultiConfigureDelay,
    decoder: &mut core::CommandDecoder,
    read: &mut [u8],
) -> io::Result<()> {
    decoder.reset();
    let mut report = multi::DeviceReport::default();
    stream.write_all(&multi::detect_frames()).await?;
    let detected = pump_report(
        stream,
        decoder,
        read,
        &mut report,
        DETECT_TIMEOUT,
        |report| {
            report.detected()
                && !report.interfaces().is_empty()
                && report.firmware_version().is_some()
        },
    )
    .await?;
    if !detected {
        let message = if !report.detected() {
            "RNodeMulti device did not answer the detect query"
        } else if report.interfaces().is_empty() {
            "RNodeMulti device did not report its radio inventory"
        } else {
            "RNodeMulti device did not report its firmware version"
        };
        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
    }
    if report.firmware_ok() != Some(true) {
        let (major, minor) = report.firmware_version().unwrap_or((0, 0));
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "RNodeMulti firmware {major}.{minor} is too old; version {}.{} or newer is required",
                multi::REQUIRED_FW_VERSION_MAJOR,
                multi::REQUIRED_FW_VERSION_MINOR
            ),
        ));
    }
    for member in members {
        let radio_type = report
            .interfaces()
            .radio_type(member.vport)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "RNodeMulti vport {} is not present; the device reported {} radio(s)",
                        member.vport.get(),
                        report.interfaces().len()
                    ),
                )
            })?;
        if !radio_type.supports(member.radio.frequency()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "RNodeMulti vport {} reports {radio_type:?}, which does not support {} Hz",
                    member.vport.get(),
                    member.radio.frequency().hz()
                ),
            ));
        }
    }
    for member in members {
        if !configure_delay.duration().is_zero() {
            tokio::time::sleep(configure_delay.duration()).await;
        }
        stream
            .write_all(&member.radio.init_command_bytes(member.vport))
            .await?;
        pump_report(
            stream,
            decoder,
            read,
            &mut report,
            VALIDATE_TIMEOUT,
            |report| report.radio(member.vport).all_validated_params_present(),
        )
        .await?;
        if !report.radio(member.vport).validates(member.radio) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "RNodeMulti vport {} reported radio parameters that do not match its configuration",
                    member.vport.get()
                ),
            ));
        }
    }
    Ok(())
}

async fn pump_report<S, Done>(
    stream: &mut S,
    decoder: &mut core::CommandDecoder,
    read: &mut [u8],
    report: &mut multi::DeviceReport,
    timeout: Duration,
    mut done: Done,
) -> io::Result<bool>
where
    S: AsyncRead + Unpin,
    Done: FnMut(&multi::DeviceReport) -> bool,
{
    if done(report) {
        return Ok(true);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let read_count = match tokio::time::timeout(remaining, stream.read(read)).await {
            Err(_) => return Ok(false),
            Ok(Ok(0)) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(Ok(read_count)) => read_count,
            Ok(Err(error)) => return Err(error),
        };
        decoder.feed_slice(&read[..read_count], |command, payload| {
            report.apply(command, payload);
        });
        if done(report) {
            return Ok(true);
        }
    }
}
