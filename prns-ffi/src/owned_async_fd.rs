//! Owned Unix descriptors registered without exposing descriptor replacement.

use std::fs::File;
use std::io;
use std::ops::Deref;
use std::os::fd::AsRawFd;

use tokio::io::unix::AsyncFd;

mod sealed {
    pub trait Sealed {}

    impl Sealed for std::fs::File {}

    #[cfg(all(target_os = "linux", feature = "linux-packet"))]
    impl Sealed for crate::ethernet::PacketSocket {}

    #[cfg(all(target_os = "linux", feature = "linux-packet"))]
    impl Sealed for crate::ethernet::LinkMonitor {}
}

/// An owned descriptor whose identity cannot change through shared access.
///
/// This trait is sealed: only the reviewed descriptor owners in this module
/// can be registered through the safe constructor.
pub trait DescriptorOwner: sealed::Sealed + AsRawFd {}

impl DescriptorOwner for File {}

#[cfg(all(target_os = "linux", feature = "linux-packet"))]
impl DescriptorOwner for crate::ethernet::PacketSocket {}

#[cfg(all(target_os = "linux", feature = "linux-packet"))]
impl DescriptorOwner for crate::ethernet::LinkMonitor {}

/// Readiness for an owned, stable Unix descriptor.
///
/// Shared access supports Tokio readiness guards, while the private inner value
/// prevents replacing or closing the registered descriptor before deregistration.
///
/// A raw descriptor does not establish ownership:
///
/// ```compile_fail
/// use prns_ffi::owned_async_fd::OwnedAsyncFd;
/// let _ = OwnedAsyncFd::new(42_i32);
/// ```
///
/// Mutable access to the registered owner is intentionally unavailable:
///
/// ```compile_fail
/// use std::fs::File;
/// use prns_ffi::owned_async_fd::OwnedAsyncFd;
/// fn replace(fd: &mut OwnedAsyncFd<File>, replacement: File) {
///     *fd.get_mut() = replacement;
/// }
/// ```
pub struct OwnedAsyncFd<T: DescriptorOwner> {
    inner: AsyncFd<T>,
}

impl<T: DescriptorOwner> OwnedAsyncFd<T> {
    /// Register an owned descriptor inside a running Tokio I/O runtime.
    pub fn new(owner: T) -> io::Result<Self> {
        // SAFETY: The sealed owners hold a valid descriptor until dropped. File
        // and the private OwnedFd fields in PacketSocket/LinkMonitor cannot
        // change that descriptor through shared access. Ownership moves into
        // the private AsyncFd, and this wrapper exposes neither mutable access
        // nor ownership of that AsyncFd. Its drop deregisters before dropping
        // the descriptor owner, as required by AsyncFd::register.
        unsafe { AsyncFd::register(owner) }
            .map(|inner| Self { inner })
            .map_err(io::Error::from)
    }
}

impl<T: DescriptorOwner> Deref for OwnedAsyncFd<T> {
    type Target = AsyncFd<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::OwnedAsyncFd;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    #[tokio::test]
    async fn owned_descriptor_stays_open_until_registration_is_dropped() -> io::Result<()> {
        let (owner, mut peer) = UnixStream::pair()?;
        owner.set_nonblocking(true)?;
        peer.set_nonblocking(true)?;
        let registered = OwnedAsyncFd::new(File::from(OwnedFd::from(owner)))?;
        let mut byte = [0];
        assert_eq!(
            peer.read(&mut byte).err().map(|e| e.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        drop(registered);
        assert_eq!(peer.read(&mut byte)?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn owned_descriptor_preserves_readiness_and_file_io() -> io::Result<()> {
        let (owner, mut peer) = UnixStream::pair()?;
        owner.set_nonblocking(true)?;
        let registered = OwnedAsyncFd::new(File::from(OwnedFd::from(owner)))?;
        peer.write_all(b"ready")?;
        let mut bytes = [0; 5];
        let count = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let mut ready = registered.readable().await?;
                match ready.try_io(|inner| inner.get_ref().read(&mut bytes)) {
                    Ok(result) => return result,
                    Err(_) => continue,
                }
            }
        })
        .await
        .map_err(io::Error::other)??;
        assert_eq!(count, 5);
        assert_eq!(&bytes, b"ready");
        Ok(())
    }
}
