use std::future::pending;
use std::net::Ipv4Addr;
use std::sync::mpsc as sync_mpsc;

use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::oneshot;

use windows::core::HSTRING;
use windows::Devices::WiFiDirect::{
    WiFiDirectAdvertisementPublisher, WiFiDirectAdvertisementPublisherStatus,
    WiFiDirectAdvertisementPublisherStatusChangedEventArgs,
};
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use prns_core::interfaces::wifi_direct::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
    WifiDirectGroup,
};
use prns_core::interfaces::wifi_direct::{
    DataPlanePlan, GoIntent, GroupRole, SegmentAddress, GROUP_PASSPHRASE, GROUP_SSID_PREFIX,
};
use prns_core::interfaces::MacAddress;

const ABORTED_REASON: &str = "Wi-Fi Direct group owner aborted (radio off or unsupported)";

pub struct WindowsWifiDirectGroup {
    owner: Ipv4Addr,
}

impl WifiDirectGroup for WindowsWifiDirectGroup {
    fn role(&self) -> GroupRole {
        GroupRole::Owner
    }

    fn data_plane(&self) -> DataPlanePlan {
        DataPlanePlan::HostRendezvous {
            local: SegmentAddress::V4(self.owner),
        }
    }
}

enum PublisherEvent {
    Started,
    Stopped,
    Aborted,
}

enum Command {
    Start,
    Stop,
}

#[derive(Debug)]
pub enum WindowsWifiDirectError {
    Setup(windows::core::Error),
    Closed,
}

impl From<windows::core::Error> for WindowsWifiDirectError {
    fn from(error: windows::core::Error) -> Self {
        Self::Setup(error)
    }
}

pub struct WindowsWifiDirectBackend {
    commands: sync_mpsc::Sender<Command>,
    events: tokio_mpsc::UnboundedReceiver<PublisherEvent>,
    hosting: bool,
}

impl WindowsWifiDirectBackend {
    pub async fn new() -> Result<Self, WindowsWifiDirectError> {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel::<PublisherEvent>();
        let (commands, command_rx) = sync_mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), WindowsWifiDirectError>>();
        let ssid = group_ssid();

        std::thread::Builder::new()
            .name("prns-wifidirect-winrt".into())
            .spawn(move || publisher_thread(ssid, events_tx, command_rx, ready_tx))
            .map_err(|_| WindowsWifiDirectError::Closed)?;

        match ready_rx.await {
            Ok(Ok(())) => Ok(Self {
                commands,
                events: events_rx,
                hosting: false,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(WindowsWifiDirectError::Closed),
        }
    }

    fn command(&self, command: Command) -> Result<(), WindowsWifiDirectError> {
        self.commands
            .send(command)
            .map_err(|_| WindowsWifiDirectError::Closed)
    }
}

impl WifiDirectBackend for WindowsWifiDirectBackend {
    type Error = WindowsWifiDirectError;
    type Group = WindowsWifiDirectGroup;

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        match mode {
            DiscoveryMode::On => self.command(Command::Start),
            DiscoveryMode::Off => Ok(()),
        }
    }

    async fn form_group(&mut self, _peer: MacAddress, _intent: GoIntent) {
        let _ = self.command(Command::Start);
    }

    async fn accept_invitation(&mut self, _peer: MacAddress, _intent: GoIntent) {
        let _ = self.command(Command::Start);
    }

    async fn remove_group(&mut self) {
        let _ = self.command(Command::Stop);
    }

    async fn next_event(&mut self) -> WifiDirectEvent<WindowsWifiDirectGroup> {
        loop {
            match self.events.recv().await {
                Some(PublisherEvent::Started) => {
                    if !self.hosting {
                        self.hosting = true;
                        return WifiDirectEvent::GroupFormed {
                            group: WindowsWifiDirectGroup {
                                owner: Ipv4Addr::UNSPECIFIED,
                            },
                        };
                    }
                }
                Some(PublisherEvent::Stopped) => {
                    if self.hosting {
                        self.hosting = false;
                        return WifiDirectEvent::GroupLost {
                            reason: GroupEndReason::LocalRemoved,
                        };
                    }
                }
                Some(PublisherEvent::Aborted) => {
                    self.hosting = false;
                    return WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(
                        ABORTED_REASON,
                    ));
                }
                None => pending().await,
            }
        }
    }
}

fn group_ssid() -> HSTRING {
    let seed = std::env::var("COMPUTERNAME").unwrap_or_default();
    let mut hash: u16 = 0x9e37;
    for byte in seed.bytes() {
        hash = hash.rotate_left(5) ^ u16::from(byte);
    }
    HSTRING::from(std::format!("{GROUP_SSID_PREFIX}{hash:04x}"))
}

fn publisher_thread(
    ssid: HSTRING,
    events_tx: tokio_mpsc::UnboundedSender<PublisherEvent>,
    command_rx: sync_mpsc::Receiver<Command>,
    ready_tx: oneshot::Sender<Result<(), WindowsWifiDirectError>>,
) {
    // SAFETY: CoIncrementMTAUsage takes no input pointers and has no caller-side memory-safety
    // preconditions. We intentionally retain its process-wide MTA reference for the lifetime of
    // this service, including any later publisher restart, so WinRT remains available.
    let _mta = match unsafe { CoIncrementMTAUsage() } {
        Ok(cookie) => cookie,
        Err(error) => {
            let _ = ready_tx.send(Err(WindowsWifiDirectError::Setup(error)));
            return;
        }
    };
    let publisher = match build_publisher(&ssid, events_tx) {
        Ok(publisher) => publisher,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };
    if ready_tx.send(Ok(())).is_err() {
        return;
    }
    while let Ok(command) = command_rx.recv() {
        let result = match command {
            Command::Start => publisher.Start(),
            Command::Stop => publisher.Stop(),
        };
        if let Err(error) = result {
            crate::diagnostic_log::warn!("wifi-direct: publisher command failed ({error:?})");
        }
    }
    let _ = publisher.Stop();
}

fn build_publisher(
    ssid: &HSTRING,
    events_tx: tokio_mpsc::UnboundedSender<PublisherEvent>,
) -> Result<WiFiDirectAdvertisementPublisher, WindowsWifiDirectError> {
    let publisher = WiFiDirectAdvertisementPublisher::new()?;
    let advertisement = publisher.Advertisement()?;
    advertisement.SetIsAutonomousGroupOwnerEnabled(true)?;

    let legacy = advertisement.LegacySettings()?;
    legacy.SetIsEnabled(true)?;
    legacy.SetSsid(ssid)?;
    legacy
        .Passphrase()?
        .SetPassword(&HSTRING::from(GROUP_PASSPHRASE))?;

    publisher.StatusChanged(&TypedEventHandler::new(
        move |_publisher: &Option<WiFiDirectAdvertisementPublisher>,
              args: &Option<WiFiDirectAdvertisementPublisherStatusChangedEventArgs>| {
            if let Some(args) = args.as_ref() {
                if let Some(event) = publisher_event_from(args.Status().ok()) {
                    let _ = events_tx.send(event);
                }
            }
            Ok(())
        },
    ))?;
    Ok(publisher)
}

fn publisher_event_from(
    status: Option<WiFiDirectAdvertisementPublisherStatus>,
) -> Option<PublisherEvent> {
    match status {
        Some(WiFiDirectAdvertisementPublisherStatus::Started) => Some(PublisherEvent::Started),
        Some(WiFiDirectAdvertisementPublisherStatus::Stopped) => Some(PublisherEvent::Stopped),
        Some(WiFiDirectAdvertisementPublisherStatus::Aborted) => Some(PublisherEvent::Aborted),
        _ => None,
    }
}
