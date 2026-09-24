use std::{
    io,
    mem::size_of,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

const RECEIVE_BYTES: usize = 16_384;
const NETLINK_HEADER_BYTES: usize = 16;
const LINK_HEADER_BYTES: usize = 16;
const MESSAGE_ALIGNMENT: usize = 4;

#[derive(Debug, PartialEq, Eq)]
pub enum LinkChange {
    Deleted { index: i32 },
    Down { index: i32 },
    Updated,
}
#[derive(Debug, PartialEq, Eq)]
pub enum LinkChanges {
    Observed(Vec<LinkChange>),
    ResyncRequired,
}
pub struct LinkMonitor(OwnedFd);
impl AsRawFd for LinkMonitor {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
impl LinkMonitor {
    pub fn new() -> io::Result<Self> {
        // SAFETY: all arguments are scalar; Linux creates a fresh descriptor.
        let raw = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: socket transferred ownership of this successful descriptor once.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: zero initializes valid sockaddr_nl integer fields including reserved padding.
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as u16;
        address.nl_groups = libc::RTMGRP_LINK as u32;
        // SAFETY: initialized sockaddr_nl storage stays live throughout bind.
        if unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_nl).cast(),
                size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }
    pub fn receive(&self) -> io::Result<LinkChanges> {
        let mut bytes = [0u8; RECEIVE_BYTES];
        // SAFETY: sockaddr_nl consists of integer fields with valid zero values.
        let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut sender_length = size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        // SAFETY: both writable buffers have their advertised capacity; MSG_TRUNC never writes beyond it.
        let length = unsafe {
            libc::recvfrom(
                self.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_TRUNC,
                (&mut sender as *mut libc::sockaddr_nl).cast(),
                &mut sender_length,
            )
        };
        if length < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ENOBUFS) {
                Ok(LinkChanges::ResyncRequired)
            } else {
                Err(error)
            };
        }
        if sender_length as usize != size_of::<libc::sockaddr_nl>()
            || sender.nl_family != libc::AF_NETLINK as u16
            || sender.nl_pid != 0
        {
            return Ok(LinkChanges::Observed(Vec::new()));
        }
        if length as usize > bytes.len() {
            return Ok(LinkChanges::ResyncRequired);
        }
        if length == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        Ok(parse(&bytes[..length as usize]))
    }
}

fn parse(mut bytes: &[u8]) -> LinkChanges {
    let mut changes = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < NETLINK_HEADER_BYTES {
            return LinkChanges::ResyncRequired;
        }
        let length = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let kind = u16::from_ne_bytes([bytes[4], bytes[5]]);
        if length < NETLINK_HEADER_BYTES || length > bytes.len() {
            return LinkChanges::ResyncRequired;
        }
        if kind == libc::NLMSG_OVERRUN as u16 || kind == libc::NLMSG_ERROR as u16 {
            return LinkChanges::ResyncRequired;
        }
        if kind == libc::RTM_NEWLINK || kind == libc::RTM_DELLINK {
            if length < NETLINK_HEADER_BYTES + LINK_HEADER_BYTES {
                return LinkChanges::ResyncRequired;
            }
            let link = &bytes[NETLINK_HEADER_BYTES..length];
            let index = i32::from_ne_bytes([link[4], link[5], link[6], link[7]]);
            let flags = u32::from_ne_bytes([link[8], link[9], link[10], link[11]]);
            changes.push(if kind == libc::RTM_DELLINK {
                LinkChange::Deleted { index }
            } else if flags & libc::IFF_UP as u32 == 0 {
                LinkChange::Down { index }
            } else {
                LinkChange::Updated
            });
        }
        let aligned = (length + MESSAGE_ALIGNMENT - 1) & !(MESSAGE_ALIGNMENT - 1);
        if length == bytes.len() {
            break;
        }
        let Some(rest) = bytes.get(aligned..) else {
            return LinkChanges::ResyncRequired;
        };
        bytes = rest;
    }
    LinkChanges::Observed(changes)
}

#[cfg(test)]
mod tests;
