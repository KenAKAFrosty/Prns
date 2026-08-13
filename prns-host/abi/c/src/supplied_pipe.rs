use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

use crate::readiness::{Readiness, ReadinessCallback, RegisteredReadiness};
use crate::{
    catch_status, issued_command, lock, required_mut, required_ref, status, PrnsIssuedCommand,
    PrnsReadinessRegistration, PrnsStringView,
};
use prns_host_core::{Status as AbiStatus, SAFE_UINT_MAX};

pub struct PrnsSuppliedPipe {
    #[cfg(unix)]
    native: prns_host_native::NativeSuppliedPipe,
    attachment_readiness: Arc<Readiness>,
    request_readiness: Arc<Readiness>,
    readiness_registration: Mutex<Option<Arc<RegisteredReadiness>>>,
}

impl Drop for PrnsSuppliedPipe {
    fn drop(&mut self) {
        let registration = lock(&self.readiness_registration).take();
        if let Some(registration) = registration {
            self.request_readiness.unregister(&registration);
        }
        #[cfg(unix)]
        self.native.close();
    }
}

pub struct PrnsSuppliedPipeOpenRequest {
    #[cfg(unix)]
    native: prns_host_native::SuppliedPipeOpenRequest,
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_attach_supplied_pipe(
    host: *mut crate::PrnsHost,
    name: PrnsStringView,
    respawn_delay_millis: u64,
    bitrate_kind: u32,
    bitrate_bps: u64,
    out_value: *mut *mut PrnsSuppliedPipe,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        let name = match unsafe { crate::read_string(name) } {
            Ok(name) if !name.is_empty() => name.to_string(),
            Ok(_) => return status(AbiStatus::InvalidArgument),
            Err(error) => return error,
        };
        if respawn_delay_millis > SAFE_UINT_MAX {
            return status(AbiStatus::InvalidArgument);
        }
        let bitrate = match crate::parse_bitrate(bitrate_kind, bitrate_bps) {
            Ok(bitrate) => bitrate,
            Err(error) => return error,
        };
        #[cfg(not(unix))]
        {
            let _ = (host, name, respawn_delay_millis, bitrate);
            status(AbiStatus::Unsupported)
        }
        #[cfg(unix)]
        {
            let attachment_readiness = Arc::new(Readiness::new());
            let attachment_signal = weak_signal(&attachment_readiness);
            let request_readiness = Arc::new(Readiness::new());
            let request_signal = weak_signal(&request_readiness);
            let native = lock(&host.native);
            let Some(native) = native.as_ref() else {
                return status(AbiStatus::Stopped);
            };
            let supplied = match native.begin_supplied_pipe(
                prns_host_native::SuppliedPipeConfig {
                    name,
                    respawn_delay: Duration::from_millis(respawn_delay_millis),
                    bitrate,
                },
                Some(attachment_signal),
                Some(request_signal),
            ) {
                Ok(supplied) => supplied,
                Err(prns_host_native::NativeSubmitError::Busy) => {
                    return status(AbiStatus::QueueFull)
                }
                Err(prns_host_native::NativeSubmitError::Stopped) => {
                    return status(AbiStatus::Stopped)
                }
            };
            *out = Box::into_raw(Box::new(PrnsSuppliedPipe {
                native: supplied,
                attachment_readiness,
                request_readiness,
                readiness_registration: Mutex::new(None),
            }));
            status(AbiStatus::Ok)
        }
    })
}

fn weak_signal(readiness: &Arc<Readiness>) -> prns_host_native::ReadinessSignal {
    let readiness = Arc::downgrade(readiness);
    Arc::new(move || {
        if let Some(readiness) = readiness.upgrade() {
            readiness.notify();
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_claim_attachment(
    supplied_pipe: *mut PrnsSuppliedPipe,
    out_value: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let supplied_pipe = match unsafe { required_ref(supplied_pipe) } {
            Ok(supplied_pipe) => supplied_pipe,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        #[cfg(not(unix))]
        {
            let _ = supplied_pipe;
            status(AbiStatus::Unsupported)
        }
        #[cfg(unix)]
        {
            let Some(attachment) = supplied_pipe.native.claim_attachment() else {
                return status(AbiStatus::AlreadyClaimed);
            };
            *out = issued_command(attachment, Arc::clone(&supplied_pipe.attachment_readiness));
            status(AbiStatus::Ok)
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_next_open_request(
    supplied_pipe: *mut PrnsSuppliedPipe,
    timeout_millis: u32,
    out_value: *mut *mut PrnsSuppliedPipeOpenRequest,
) -> u32 {
    catch_status(|| {
        let supplied_pipe = match unsafe { required_ref(supplied_pipe) } {
            Ok(supplied_pipe) => supplied_pipe,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        #[cfg(not(unix))]
        {
            let _ = (supplied_pipe, timeout_millis);
            status(AbiStatus::Unsupported)
        }
        #[cfg(unix)]
        {
            let timeout = if timeout_millis == crate::NEVER_TIMEOUT {
                None
            } else {
                Some(Duration::from_millis(u64::from(timeout_millis)))
            };
            match supplied_pipe.native.next_request(timeout) {
                prns_host_native::SuppliedPipeRequestWait::Request(request) => {
                    *out = Box::into_raw(Box::new(PrnsSuppliedPipeOpenRequest { native: request }));
                    status(AbiStatus::Ok)
                }
                prns_host_native::SuppliedPipeRequestWait::WouldBlock => {
                    status(AbiStatus::WouldBlock)
                }
                prns_host_native::SuppliedPipeRequestWait::TimedOut => status(AbiStatus::TimedOut),
                prns_host_native::SuppliedPipeRequestWait::Interrupted => {
                    status(AbiStatus::Interrupted)
                }
                prns_host_native::SuppliedPipeRequestWait::Stopped => status(AbiStatus::Stopped),
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_register_readiness(
    supplied_pipe: *mut PrnsSuppliedPipe,
    callback: Option<ReadinessCallback>,
    context: *mut c_void,
    out_value: *mut *mut PrnsReadinessRegistration,
) -> u32 {
    catch_status(|| {
        let supplied_pipe = match unsafe { required_ref(supplied_pipe) } {
            Ok(supplied_pipe) => supplied_pipe,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        let Some(callback) = callback else {
            return status(AbiStatus::InvalidArgument);
        };
        let readiness = Arc::clone(&supplied_pipe.request_readiness);
        let registered = match readiness.register(callback, context) {
            Ok(registered) => registered,
            Err(_) => return status(AbiStatus::AlreadyClaimed),
        };
        *lock(&supplied_pipe.readiness_registration) = Some(Arc::clone(&registered));
        *out = Box::into_raw(Box::new(PrnsReadinessRegistration {
            readiness,
            registered,
        }));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_interrupt_wait(supplied_pipe: *mut PrnsSuppliedPipe) {
    if let Ok(Some(supplied_pipe)) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            supplied_pipe.as_ref()
        }))
    {
        #[cfg(unix)]
        supplied_pipe.native.interrupt_wait();
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_release(supplied_pipe: *mut PrnsSuppliedPipe) {
    if !supplied_pipe.is_null() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(supplied_pipe));
        }));
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_open_request_provide(
    supplied_pipe_open_request: *mut PrnsSuppliedPipeOpenRequest,
    descriptor: i64,
    out_value: *mut u8,
) -> u32 {
    catch_status(|| {
        let request = match unsafe { required_ref(supplied_pipe_open_request) } {
            Ok(request) => request,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = 0;
        if descriptor < 0 || descriptor > i64::from(i32::MAX) {
            return status(AbiStatus::InvalidArgument);
        }
        #[cfg(not(unix))]
        {
            let _ = request;
            status(AbiStatus::Unsupported)
        }
        #[cfg(unix)]
        {
            let raw = descriptor as RawFd;
            // SAFETY: Argument validation established the platform descriptor
            // range. A successful ABI call consumes this descriptor exactly once.
            let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
            if set_nonblocking(raw).is_err() {
                drop(descriptor);
                let _ = request.native.decline();
                return status(AbiStatus::Ok);
            }
            *out = u8::from(request.native.provide(descriptor));
            status(AbiStatus::Ok)
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_open_request_decline(
    supplied_pipe_open_request: *mut PrnsSuppliedPipeOpenRequest,
    out_value: *mut u8,
) -> u32 {
    catch_status(|| {
        let request = match unsafe { required_ref(supplied_pipe_open_request) } {
            Ok(request) => request,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        #[cfg(not(unix))]
        {
            let _ = request;
            *out = 0;
            status(AbiStatus::Unsupported)
        }
        #[cfg(unix)]
        {
            *out = u8::from(request.native.decline());
            status(AbiStatus::Ok)
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_supplied_pipe_open_request_release(
    supplied_pipe_open_request: *mut PrnsSuppliedPipeOpenRequest,
) {
    if !supplied_pipe_open_request.is_null() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(supplied_pipe_open_request));
        }));
    }
}

#[cfg(unix)]
fn set_nonblocking(descriptor: RawFd) -> Result<(), ()> {
    // SAFETY: The descriptor is owned by this call and neither invocation
    // retains it.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(());
    }
    // SAFETY: Existing flags are preserved and the descriptor remains owned.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::mem::size_of;
    use std::os::fd::IntoRawFd;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Instant;

    use crate::{
        prns_command_release, prns_command_wait, prns_readiness_registration_release,
        AbiBitrateKind, AbiCommandOutcomeKind, CoreLimits, HostConfig, HostRole, IdentityConfig,
        NativeHost, PersistenceConfig, PrnsCommandResult,
    };

    unsafe extern "C" fn count_readiness(context: *mut c_void) {
        // SAFETY: The test keeps the counter alive until registration release.
        let counter = unsafe { &*context.cast::<AtomicUsize>() };
        counter.fetch_add(1, Ordering::AcqRel);
    }

    fn native_host() -> Result<crate::PrnsHost, String> {
        let (host, publisher) = crate::host_capsule(CoreLimits::balanced());
        let native = NativeHost::start(
            HostConfig {
                identity: IdentityConfig::GenerateEphemeral,
                persistence: PersistenceConfig::Ephemeral,
                role: HostRole::Endpoint,
                destinations: Vec::new(),
                required_capabilities: Vec::new(),
                limits: CoreLimits::balanced(),
            },
            Arc::new(publisher),
        )
        .map_err(|error| format!("{error:?}"))?;
        *lock(&host.native) = Some(native);
        Ok(host)
    }

    #[test]
    fn pull_contract_covers_claim_readiness_descriptor_and_interrupt() -> Result<(), String> {
        let mut host = native_host()?;
        let name = b"c-pull-contract";
        let mut pipe = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_host_attach_supplied_pipe(
                    &mut host,
                    PrnsStringView {
                        data: name.as_ptr(),
                        length: name.len(),
                    },
                    50,
                    AbiBitrateKind::Auto as u32,
                    0,
                    &mut pipe,
                )
            },
            status(AbiStatus::Ok)
        );
        let mut command = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_claim_attachment(pipe, &mut command) },
            status(AbiStatus::Ok)
        );
        let mut duplicate = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_claim_attachment(pipe, &mut duplicate) },
            status(AbiStatus::AlreadyClaimed)
        );
        let mut result = PrnsCommandResult {
            struct_size: size_of::<PrnsCommandResult>(),
            outcome: 0,
            failure: 0,
            evidence: 0,
            rtt_millis: 0,
            value: crate::PrnsByteView {
                data: ptr::null(),
                length: 0,
            },
            detail: PrnsStringView {
                data: ptr::null(),
                length: 0,
            },
        };
        assert_eq!(
            unsafe { prns_command_wait(command, 2_000, &mut result) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            result.outcome,
            AbiCommandOutcomeKind::InterfaceAttached as u32
        );

        let mut first = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_next_open_request(pipe, 2_000, &mut first) },
            status(AbiStatus::Ok)
        );
        let readiness_count = AtomicUsize::new(0);
        let mut registration = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_supplied_pipe_register_readiness(
                    pipe,
                    Some(count_readiness),
                    ptr::from_ref(&readiness_count).cast_mut().cast(),
                    &mut registration,
                )
            },
            status(AbiStatus::Ok)
        );
        let mut declined = 0;
        assert_eq!(
            unsafe { prns_supplied_pipe_open_request_decline(first, &mut declined) },
            status(AbiStatus::Ok)
        );
        assert_eq!(declined, 1);
        unsafe { prns_supplied_pipe_open_request_release(first) };
        let mut absent = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_next_open_request(pipe, 0, &mut absent) },
            status(AbiStatus::WouldBlock)
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while readiness_count.load(Ordering::Acquire) == 0 {
            if Instant::now() >= deadline {
                return Err("the replacement request did not signal readiness".to_string());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut second = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_next_open_request(pipe, 2_000, &mut second) },
            status(AbiStatus::Ok)
        );
        let (wire, peer) =
            std::os::unix::net::UnixStream::pair().map_err(|error| error.to_string())?;
        let mut accepted = 0;
        assert_eq!(
            unsafe {
                prns_supplied_pipe_open_request_provide(
                    second,
                    i64::from(wire.into_raw_fd()),
                    &mut accepted,
                )
            },
            status(AbiStatus::Ok)
        );
        assert_eq!(accepted, 1);
        unsafe { prns_supplied_pipe_open_request_release(second) };

        unsafe { prns_supplied_pipe_interrupt_wait(pipe) };
        let mut interrupted = ptr::null_mut();
        assert_eq!(
            unsafe { prns_supplied_pipe_next_open_request(pipe, 2_000, &mut interrupted) },
            status(AbiStatus::Interrupted)
        );
        drop(peer);
        unsafe {
            prns_readiness_registration_release(registration);
            prns_command_release(command);
            prns_supplied_pipe_release(pipe);
        }
        Ok(())
    }
}
