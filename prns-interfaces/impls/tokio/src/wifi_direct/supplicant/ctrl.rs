use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::net::UnixDatagram;

const REPLY_CAPACITY: usize = 8192;

static CLIENT_SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Debug)]
pub enum WpaCtrlError {
    Bind(io::Error),
    Connect(io::Error),
    Send(io::Error),
    Receive(io::Error),
    AttachRejected,
}

pub struct WpaEvent {
    pub name: String,
    pub payload: String,
}

impl WpaEvent {
    fn parse(datagram: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(datagram).ok()?;
        let after_level = text.strip_prefix('<')?.split_once('>')?.1.trim_end();
        let (name, payload) = match after_level.split_once(' ') {
            Some((name, payload)) => (name.to_owned(), payload.to_owned()),
            None => (after_level.to_owned(), String::new()),
        };
        Some(Self { name, payload })
    }
}

fn client_socket_path(role: &str) -> PathBuf {
    let sequence = CLIENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("prns-wpa-{role}-{}-{sequence}", std::process::id()));
    path
}

fn bound_to(ctrl_socket: &Path, role: &str) -> Result<(UnixDatagram, PathBuf), WpaCtrlError> {
    let local = client_socket_path(role);
    let _ = std::fs::remove_file(&local);
    let socket = UnixDatagram::bind(&local).map_err(WpaCtrlError::Bind)?;
    socket.connect(ctrl_socket).map_err(WpaCtrlError::Connect)?;
    Ok((socket, local))
}

pub struct WpaCommand {
    socket: UnixDatagram,
    local: PathBuf,
}

impl WpaCommand {
    pub fn open(ctrl_socket: &Path) -> Result<Self, WpaCtrlError> {
        let (socket, local) = bound_to(ctrl_socket, "cmd")?;
        Ok(Self { socket, local })
    }

    pub async fn request(&self, command: &str) -> Result<String, WpaCtrlError> {
        self.socket
            .send(command.as_bytes())
            .await
            .map_err(WpaCtrlError::Send)?;
        let mut buffer = [0u8; REPLY_CAPACITY];
        loop {
            let read = self
                .socket
                .recv(&mut buffer)
                .await
                .map_err(WpaCtrlError::Receive)?;
            if buffer.first() == Some(&b'<') {
                continue;
            }
            return Ok(String::from_utf8_lossy(&buffer[..read])
                .trim_end()
                .to_owned());
        }
    }
}

impl Drop for WpaCommand {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.local);
    }
}

pub struct WpaMonitor {
    socket: UnixDatagram,
    local: PathBuf,
}

impl WpaMonitor {
    pub async fn open(ctrl_socket: &Path) -> Result<Self, WpaCtrlError> {
        let (socket, local) = bound_to(ctrl_socket, "mon")?;
        socket.send(b"ATTACH").await.map_err(WpaCtrlError::Send)?;
        let mut buffer = [0u8; 16];
        let read = socket
            .recv(&mut buffer)
            .await
            .map_err(WpaCtrlError::Receive)?;
        if buffer[..read].trim_ascii_end() != b"OK" {
            return Err(WpaCtrlError::AttachRejected);
        }
        Ok(Self { socket, local })
    }

    pub async fn next_event(&self) -> Result<WpaEvent, WpaCtrlError> {
        let mut buffer = [0u8; REPLY_CAPACITY];
        loop {
            let read = self
                .socket
                .recv(&mut buffer)
                .await
                .map_err(WpaCtrlError::Receive)?;
            if let Some(event) = WpaEvent::parse(&buffer[..read]) {
                return Ok(event);
            }
        }
    }
}

impl Drop for WpaMonitor {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.local);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_strips_its_priority_level_and_splits_name_from_payload() {
        let event = WpaEvent::parse(b"<3>P2P-DEVICE-FOUND 42:00:00:00:00:00 name='Prns'\n")
            .expect("a well-formed event parses");
        assert_eq!(event.name, "P2P-DEVICE-FOUND");
        assert_eq!(event.payload, "42:00:00:00:00:00 name='Prns'");
    }

    #[test]
    fn a_bare_event_has_an_empty_payload() {
        let event = WpaEvent::parse(b"<3>P2P-FIND-STOPPED\n").expect("parses");
        assert_eq!(event.name, "P2P-FIND-STOPPED");
        assert_eq!(event.payload, "");
    }

    #[test]
    fn a_reply_without_a_priority_level_is_not_an_event() {
        assert!(WpaEvent::parse(b"OK\n").is_none());
        assert!(WpaEvent::parse(b"DIRECT-45\n").is_none());
    }
}
