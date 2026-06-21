//! The Windows (WinRT) backend for the native Reticulum-over-BLE auto-interface.
//!
//! GATT-only by mandate: WinRT exposes no app-level L2CAP, so [`arrangement`] pins every Windows
//! pair to `GattOnly` and `upgrade()` is a permanent no-op. The GATT-data floor carries every
//! frame; this is a complete, correct backend, not a degraded one. The shared brain (discovery
//! dedup, orientation, make-before-break) lives in the engine's `BluetoothAuto` supervisor — this
//! backend only drives the radio and the seam, mirroring `personal-rns-ffi/src/ble/macos.rs`.
//!
//! WinRT activation and async completion need an initialized COM apartment, and the projected
//! objects are not all `Send`/`Sync` in a way tokio likes, so a dedicated thread owns the radio:
//! it joins the process MTA, brings the adapter up, and parks for the backend's lifetime while
//! WinRT event handlers (which fire on the system threadpool) post into a tokio channel the async
//! consumer drains — the same callback-world-to-reactor bridge the macOS backend uses with GCD.
//!
//! Built up in steps; this is the power-up scaffold. The data/control planes and the
//! advertise/scan roles arrive in the following steps, hence the reserved fields below.
#![allow(dead_code)] // TODO(ble-windows): in-progress backend; drop once advertise/scan/link land.

use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::core::{BleAddress, Control, Dialect, L2capPlan};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
};

use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Radios::RadioState;
use windows::Win32::System::Com::CoIncrementMTAUsage;

/// How long to wait for the radio thread to bring the adapter up before giving up.
const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum WindowsBleError {
    /// No Bluetooth adapter is present on this machine.
    NoAdapter,
    /// The adapter cannot act as a BLE peripheral (advertise the service), so it cannot host a link.
    PeripheralRoleUnsupported,
    /// The adapter is present but the radio is switched off (airplane mode / hardware toggle).
    RadioOff,
    /// The radio thread or a channel went away.
    Closed,
    /// The adapter did not come up within [`POWER_ON_TIMEOUT`].
    PowerOnTimeout,
    /// A control PDU exceeded `CONTROL_MAX_LEN`.
    ControlTooLarge,
    /// A data frame exceeded the negotiated link MTU.
    FrameTooLarge,
    /// An underlying WinRT call failed.
    Winrt(windows::core::Error),
}

impl From<windows::core::Error> for WindowsBleError {
    fn from(error: windows::core::Error) -> Self {
        WindowsBleError::Winrt(error)
    }
}

/// Events from the WinRT callback world into the async consumer. Populated by the advertise/scan and
/// link steps; `next_event` already drains it.
enum Event {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(WinGattLink),
}

/// One GATT link to a peer — the control connection plus the GATT-data floor. On Windows the floor
/// is the only data plane (no L2CAP). The radio-side characteristic handles arrive in later steps;
/// for now the link is the channel scaffold the planes hang off.
pub struct WinGattLink {
    address: BleAddress,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    data_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
}

impl BleLink for WinGattLink {
    type Error = WindowsBleError;
    type Source = WinGattSource;
    type Sink = WinGattSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, _msg: &Control) -> Result<(), WindowsBleError> {
        // TODO(ble-windows step 4): write the control characteristic (…28e7).
        Err(WindowsBleError::Closed)
    }

    async fn control_recv(&mut self) -> Result<Control, WindowsBleError> {
        self.control_rx.recv().await.ok_or(WindowsBleError::Closed)
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), WindowsBleError> {
        // GATT-only: WinRT has no app-level L2CAP, so the upgrade is a permanent no-op. The floor
        // carries every frame; never failing keeps the link alive (the seam contract for upgrade).
        Ok(())
    }

    fn into_data(self) -> (WinGattSource, WinGattSink) {
        (
            WinGattSource { rx: self.data_rx },
            WinGattSink {
                address: self.address,
            },
        )
    }
}

/// The receive half of the data floor: reassembled frames arriving on the data characteristic.
pub struct WinGattSource {
    rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
}

impl BleSource for WinGattSource {
    type Error = WindowsBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, WindowsBleError> {
        let rx = self.rx.as_mut().ok_or(WindowsBleError::Closed)?;
        let frame = rx.recv().await.ok_or(WindowsBleError::Closed)?;
        let len = frame.len().min(out.len());
        let dst = out.get_mut(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        let src = frame.get(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        dst.copy_from_slice(src);
        Ok(len)
    }
}

/// The send half of the data floor: fragments a frame across writes to the data characteristic.
pub struct WinGattSink {
    address: BleAddress,
}

impl BleSink for WinGattSink {
    type Error = WindowsBleError;

    async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), WindowsBleError> {
        // TODO(ble-windows step 5): fragment + write the data-floor characteristic (…28e8).
        Err(WindowsBleError::Closed)
    }
}

/// The Windows native-BLE backend handed to the engine's `BluetoothAuto` supervisor.
pub struct WindowsBleBackend {
    /// Dropping this signals the radio thread to unpark and tear the WinRT objects down.
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
}

impl WindowsBleBackend {
    /// Bring the WinRT adapter up on a dedicated MTA thread and return once it is advertising-capable
    /// and the radio is on. Fails (so the node runs without BLE) if there is no adapter, the radio is
    /// off, or the peripheral role is unsupported.
    pub async fn new() -> Result<Self, WindowsBleError> {
        // The sender stays unused until the advertise/scan steps wire callbacks to it; holding it
        // keeps the channel open so `next_event` parks rather than seeing an immediate close.
        let (_events_tx, events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), WindowsBleError>>();

        std::thread::Builder::new()
            .name("prns-ble-winrt".into())
            .spawn(move || {
                let _ = ready_tx.send(winrt_power_on());
                // Park so the MTA (and, in later steps, the WinRT radio objects) outlive setup.
                let _ = shutdown_rx.recv();
            })
            .map_err(|_| WindowsBleError::Closed)?;

        match tokio::time::timeout(POWER_ON_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(()))) => Ok(Self {
                _keepalive: keepalive,
                events: events_rx,
            }),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(WindowsBleError::Closed),
            Err(_) => Err(WindowsBleError::PowerOnTimeout),
        }
    }
}

/// Run on the radio thread: join the MTA and verify the adapter is a powered-on BLE peripheral.
fn winrt_power_on() -> Result<(), WindowsBleError> {
    // WinRT factory activation and async completion need an initialized apartment. CoIncrementMTAUsage
    // joins (or starts) the process-wide implicit MTA and keeps it alive for this thread's lifetime
    // with no matching decrement — the right shape for a long-lived radio thread.
    // SAFETY: a plain COM call with no preconditions; the returned cookie only matters if we wanted
    // to later decrement MTA usage, which a lifelong radio thread never does.
    unsafe {
        CoIncrementMTAUsage()?;
    }

    let adapter: BluetoothAdapter = BluetoothAdapter::GetDefaultAsync()?.get()?;
    if !adapter.IsLowEnergySupported()? {
        return Err(WindowsBleError::PeripheralRoleUnsupported);
    }
    if !adapter.IsPeripheralRoleSupported()? {
        return Err(WindowsBleError::PeripheralRoleUnsupported);
    }
    let radio = adapter.GetRadioAsync()?.get()?;
    if radio.State()? != RadioState::On {
        return Err(WindowsBleError::RadioOff);
    }

    log::info!("bluetooth: WinRT adapter powered on, LE + peripheral role supported");
    Ok(())
}

impl BleBackend for WindowsBleBackend {
    const MAX_PEERS: usize = 8;
    type Error = WindowsBleError;
    type Link = WinGattLink;

    async fn set_advertising(&mut self, _enabled: bool) -> Result<(), WindowsBleError> {
        // TODO(ble-windows step 2): start/stop the GattServiceProvider advertisement.
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<WinGattLink> {
        loop {
            match self.events.recv().await {
                Some(Event::Sighting { address, rssi }) => {
                    return BleEvent::Sighting { address, rssi }
                }
                Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                // Channel closed (no senders yet in this step): nothing more will arrive, so park
                // rather than spin returning a closed signal the supervisor cannot act on.
                None => core::future::pending().await,
            }
        }
    }

    async fn dial(&mut self, _address: BleAddress) {
        // TODO(ble-windows step 4): connect, discover characteristics, build a WinGattLink.
    }
}
