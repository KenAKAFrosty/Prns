use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct HostSerial {
    #[cfg(not(windows))]
    inner: serial2_tokio::SerialPort,
    #[cfg(windows)]
    inner: windows_bridge::ThreadedSerial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbSerialPort {
    path: String,
    incarnation: String,
}

impl UsbSerialPort {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSerialDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSerialParity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSerialStopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSerialLineSettings {
    baud: u32,
    data_bits: HostSerialDataBits,
    parity: HostSerialParity,
    stop_bits: HostSerialStopBits,
}

impl HostSerialLineSettings {
    pub const fn new(
        baud: u32,
        data_bits: HostSerialDataBits,
        parity: HostSerialParity,
        stop_bits: HostSerialStopBits,
    ) -> Self {
        Self {
            baud,
            data_bits,
            parity,
            stop_bits,
        }
    }

    pub const fn eight_n_one(baud: u32) -> Self {
        Self::new(
            baud,
            HostSerialDataBits::Eight,
            HostSerialParity::None,
            HostSerialStopBits::One,
        )
    }

    pub const fn baud(self) -> u32 {
        self.baud
    }

    pub const fn data_bits(self) -> HostSerialDataBits {
        self.data_bits
    }

    pub const fn parity(self) -> HostSerialParity {
        self.parity
    }

    pub const fn stop_bits(self) -> HostSerialStopBits {
        self.stop_bits
    }
}

pub fn open_host_serial(path: &str, baud: u32) -> io::Result<HostSerial> {
    open_host_serial_with_settings(path, HostSerialLineSettings::eight_n_one(baud))
}

/// Open `path` with explicit line settings using the reliable transport for the platform.
#[cfg(not(windows))]
pub fn open_host_serial_with_settings(
    path: &str,
    settings: HostSerialLineSettings,
) -> io::Result<HostSerial> {
    use serial2_tokio::{CharSize, Parity, SerialPort, StopBits};

    let inner = SerialPort::open(path, |mut port: serial2_tokio::Settings| {
        port.set_raw();
        port.set_baud_rate(settings.baud)?;
        port.set_char_size(match settings.data_bits {
            HostSerialDataBits::Five => CharSize::Bits5,
            HostSerialDataBits::Six => CharSize::Bits6,
            HostSerialDataBits::Seven => CharSize::Bits7,
            HostSerialDataBits::Eight => CharSize::Bits8,
        });
        port.set_parity(match settings.parity {
            HostSerialParity::None => Parity::None,
            HostSerialParity::Even => Parity::Even,
            HostSerialParity::Odd => Parity::Odd,
        });
        port.set_stop_bits(match settings.stop_bits {
            HostSerialStopBits::One => StopBits::One,
            HostSerialStopBits::Two => StopBits::Two,
        });
        Ok(port)
    })?;
    Ok(HostSerial { inner })
}

#[cfg(windows)]
pub fn open_host_serial_with_settings(
    path: &str,
    settings: HostSerialLineSettings,
) -> io::Result<HostSerial> {
    windows_bridge::open(path, settings).map(|inner| HostSerial { inner })
}

impl AsyncRead for HostSerial {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for HostSerial {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub fn scan_usb_serial_ports() -> io::Result<Vec<UsbSerialPort>> {
    #[cfg(target_os = "linux")]
    {
        scan_linux_usb_serial_ports(std::path::Path::new("/sys"), std::path::Path::new("/dev"))
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        prns_ffi::usb_serial::available_ports().map(|ports| {
            ports
                .into_iter()
                .map(|port| UsbSerialPort {
                    path: port.path().to_string(),
                    incarnation: port.incarnation().to_string(),
                })
                .collect()
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(target_os = "linux")]
fn scan_linux_usb_serial_ports(
    sys_root: &std::path::Path,
    dev_root: &std::path::Path,
) -> io::Result<Vec<UsbSerialPort>> {
    use std::os::unix::fs::MetadataExt;

    let tty_root = sys_root.join("class/tty");
    let entries = match std::fs::read_dir(&tty_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ports = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(device) = entry.path().join("device").canonicalize() else {
            continue;
        };
        let usb_backed = device.ancestors().any(|ancestor| {
            ancestor.join("idVendor").is_file() && ancestor.join("idProduct").is_file()
        });
        if !usb_backed {
            continue;
        }
        let path = dev_root.join(entry.file_name());
        let Ok(metadata) = device.metadata() else {
            continue;
        };
        if !path.exists() {
            continue;
        }
        let incarnation = format!(
            "{}:{}:{}:{}",
            device.display(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        );
        ports.push(UsbSerialPort {
            path: path.to_string_lossy().into_owned(),
            incarnation,
        });
    }
    ports.sort_by(|left, right| left.path.cmp(&right.path));
    ports.dedup_by(|left, right| left.path == right.path);
    Ok(ports)
}

#[cfg(windows)]
mod windows_bridge {
    use std::io::{self, Read, Write};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::mpsc;

    const READ_CHUNK: usize = 512;
    pub struct ThreadedSerial {
        inbound: mpsc::UnboundedReceiver<io::Result<Vec<u8>>>,
        outbound: mpsc::UnboundedSender<Vec<u8>>,
        chunk: Vec<u8>,
        offset: usize,
        eof: bool,
    }

    pub fn open(path: &str, settings: super::HostSerialLineSettings) -> io::Result<ThreadedSerial> {
        let port = prns_ffi::serial::open(
            path,
            settings.baud(),
            match settings.data_bits() {
                super::HostSerialDataBits::Five => 5,
                super::HostSerialDataBits::Six => 6,
                super::HostSerialDataBits::Seven => 7,
                super::HostSerialDataBits::Eight => 8,
            },
            match settings.parity() {
                super::HostSerialParity::None => prns_ffi::serial::Parity::None,
                super::HostSerialParity::Even => prns_ffi::serial::Parity::Even,
                super::HostSerialParity::Odd => prns_ffi::serial::Parity::Odd,
            },
            match settings.stop_bits() {
                super::HostSerialStopBits::One => prns_ffi::serial::StopBits::One,
                super::HostSerialStopBits::Two => prns_ffi::serial::StopBits::Two,
            },
        )?;
        let (in_tx, in_rx) = mpsc::unbounded_channel::<io::Result<Vec<u8>>>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || io_loop(port, in_tx, out_rx));
        Ok(ThreadedSerial {
            inbound: in_rx,
            outbound: out_tx,
            chunk: Vec::new(),
            offset: 0,
            eof: false,
        })
    }

    fn io_loop(
        mut port: prns_ffi::serial::WindowsSerial,
        in_tx: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
        mut out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            loop {
                match out_rx.try_recv() {
                    Ok(data) => {
                        if port.write_all(&data).is_err() {
                            return;
                        }
                        let _ = port.flush();
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => return,
                }
            }
            match port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    if in_tx.send(Ok(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => {
                    let _ = in_tx.send(Err(error));
                    return;
                }
            }
        }
    }

    impl AsyncRead for ThreadedSerial {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            if this.offset < this.chunk.len() {
                let take = (this.chunk.len() - this.offset).min(buf.remaining());
                buf.put_slice(&this.chunk[this.offset..this.offset + take]);
                this.offset += take;
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            match this.inbound.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.chunk = chunk;
                    this.offset = 0;
                    let take = this.chunk.len().min(buf.remaining());
                    buf.put_slice(&this.chunk[..take]);
                    this.offset = take;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Some(Err(error))) => Poll::Ready(Err(error)),
                Poll::Ready(None) => {
                    this.eof = true;
                    Poll::Ready(Ok(()))
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl AsyncWrite for ThreadedSerial {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            match self.get_mut().outbound.send(buf.to_vec()) {
                Ok(()) => Poll::Ready(Ok(buf.len())),
                Err(_) => Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe))),
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::scan_linux_usb_serial_ports;
    use std::fs;

    #[test]
    fn linux_scan_returns_only_usb_backed_ttys() {
        let fixture = tempfile::tempdir().unwrap();
        let sys = fixture.path().join("sys");
        let dev = fixture.path().join("dev");
        let usb = sys.join("devices/usb1/1-1");
        let platform = sys.join("devices/platform/serial0");
        let tty = sys.join("class/tty");
        fs::create_dir_all(&usb).unwrap();
        fs::create_dir_all(&platform).unwrap();
        fs::create_dir_all(&tty).unwrap();
        fs::create_dir_all(&dev).unwrap();
        fs::write(usb.join("idVendor"), "303a").unwrap();
        fs::write(usb.join("idProduct"), "1001").unwrap();
        fs::create_dir_all(tty.join("ttyACM0")).unwrap();
        fs::create_dir_all(tty.join("ttyS0")).unwrap();
        std::os::unix::fs::symlink(&usb, tty.join("ttyACM0/device")).unwrap();
        std::os::unix::fs::symlink(&platform, tty.join("ttyS0/device")).unwrap();
        fs::write(dev.join("ttyACM0"), []).unwrap();
        fs::write(dev.join("ttyS0"), []).unwrap();

        let ports = scan_linux_usb_serial_ports(&sys, &dev).unwrap();
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].path(), dev.join("ttyACM0").to_string_lossy());
        assert!(!ports[0].incarnation().is_empty());
    }

    #[test]
    fn linux_scan_distinguishes_a_reused_tty_path() {
        let fixture = tempfile::tempdir().unwrap();
        let sys = fixture.path().join("sys");
        let dev = fixture.path().join("dev");
        let usb = sys.join("devices/usb1/1-1");
        let retired_usb = sys.join("devices/usb1/retired");
        let tty = sys.join("class/tty/ttyACM0");
        fs::create_dir_all(&usb).unwrap();
        fs::write(usb.join("idVendor"), "303a").unwrap();
        fs::write(usb.join("idProduct"), "1001").unwrap();
        fs::create_dir_all(&tty).unwrap();
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("ttyACM0"), []).unwrap();
        std::os::unix::fs::symlink(&usb, tty.join("device")).unwrap();

        let first = scan_linux_usb_serial_ports(&sys, &dev).unwrap();
        fs::rename(&usb, &retired_usb).unwrap();
        fs::create_dir_all(&usb).unwrap();
        fs::write(usb.join("idVendor"), "303a").unwrap();
        fs::write(usb.join("idProduct"), "1001").unwrap();
        let second = scan_linux_usb_serial_ports(&sys, &dev).unwrap();

        assert_eq!(first[0].path(), second[0].path());
        assert_ne!(first[0].incarnation(), second[0].incarnation());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod io_tests {
    use super::HostSerial;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn opaque_serial_stream_round_trips_over_a_pty() {
        let (inner, mut peer) = serial2_tokio::SerialPort::pair().unwrap();
        let mut host = HostSerial { inner };

        host.write_all(b"host-to-peer").await.unwrap();
        let mut from_host = [0; 12];
        peer.read_exact(&mut from_host).await.unwrap();
        assert_eq!(&from_host, b"host-to-peer");

        peer.write_all(b"peer-to-host").await.unwrap();
        let mut from_peer = [0; 12];
        host.read_exact(&mut from_peer).await.unwrap();
        assert_eq!(&from_peer, b"peer-to-host");
    }
}
