use prns_core::interfaces::MacAddress;
use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkDeviceBinding {
    index: i32,
    address: MacAddress,
}
impl NetworkDeviceBinding {
    pub fn index(self) -> i32 {
        self.index
    }
    pub fn address(self) -> MacAddress {
        self.address
    }
}

pub struct NetworkDeviceName(CString);
impl NetworkDeviceName {
    pub fn new(name: &str) -> io::Result<Self> {
        if name.is_empty() || name.len() >= libc::IFNAMSIZ {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        CString::new(name)
            .map(Self)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }
    pub fn binding(&self) -> io::Result<NetworkDeviceBinding> {
        // SAFETY: the owned name is NUL terminated and is not retained by libc.
        let index = unsafe { libc::if_nametoindex(self.0.as_ptr()) };
        if index == 0 {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let index =
            i32::try_from(index).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
        // SAFETY: socket accepts scalar arguments and returns a newly owned descriptor.
        let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful socket call transfers this descriptor once.
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: ifreq is a C POD with integer/byte/union storage, all zero valid.
        let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
        for (slot, byte) in request.ifr_name.iter_mut().zip(self.0.as_bytes_with_nul()) {
            *slot = *byte as libc::c_char;
        }
        // SAFETY: request is writable ifreq storage; Linux writes the flags union field.
        if unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCGIFFLAGS as _, &mut request) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: SIOCGIFFLAGS just initialized this union field.
        let flags = unsafe { request.ifr_ifru.ifru_flags };
        if i32::from(flags) & libc::IFF_UP == 0 {
            return Err(io::Error::from(io::ErrorKind::NetworkDown));
        }
        // SAFETY: request is writable ifreq storage; Linux writes the hardware-address field.
        if unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCGIFHWADDR as _, &mut request) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: SIOCGIFHWADDR just initialized this union field.
        let address = unsafe { request.ifr_ifru.ifru_hwaddr };
        if address.sa_family != libc::ARPHRD_ETHER {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let mut octets = [0; 6];
        for (out, byte) in octets.iter_mut().zip(address.sa_data) {
            *out = byte as u8;
        }
        Ok(NetworkDeviceBinding {
            index,
            address: MacAddress::new(octets),
        })
    }
}
