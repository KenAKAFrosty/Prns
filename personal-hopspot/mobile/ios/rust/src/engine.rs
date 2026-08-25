use personal_rns::runtime::NoPersistence;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use personal_hopspot_core::{
    card_label, load_host_ble_identity, load_host_node_identity, CardKind, CardLabel,
    HopspotDestinationSet, IdentityBootstrap, IdentityPersistence, MobileEngineFailure,
    MobileEngineState, BLE_IDENTITY_STORAGE, NODE_IDENTITY_STORAGE,
};
use personal_rns::bluetooth_auto::{AutoBle, BluetoothAutoStatus};
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::manifold::tokio::TokioInterfaceStatus;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, NodeRunError, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;
#[cfg(target_os = "ios")]
use personal_rns::wifi_auto::apple_service_discovery;
use personal_rns::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::wire::DestinationHash;
use tokio::net::TcpListener;

use crate::persistence::{PersistenceShutdown, PreparedPersistence};

const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const PERSISTENCE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);
const DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(60);
const MEMORY_DIAGNOSTIC_INTERVALS: u64 = 60;
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([
    InterfaceKind::UsbAutoDevice as u8,
    b'i',
    b'o',
    b's',
    b'-',
    b'u',
    b's',
    b'b',
]);

#[derive(Clone)]
struct EngineRuntime {
    handle: PrnsNodeHandle,
    usb_status: TokioInterfaceStatus,
    wifi_status: AutoWifiStatus,
    ble_status: Option<BluetoothAutoStatus>,
    node_page_destination: DestinationHash,
}

struct Worker {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

#[derive(Default)]
struct SupervisorInner {
    storage_directory: Option<PathBuf>,
    worker: Option<Worker>,
    runtime: Option<EngineRuntime>,
}

struct EngineSupervisor {
    state: AtomicI32,
    last_failure: AtomicI32,
    inner: Mutex<SupervisorInner>,
}

impl EngineSupervisor {
    fn new() -> Self {
        Self {
            state: AtomicI32::new(MobileEngineState::Stopped.code()),
            last_failure: AtomicI32::new(MobileEngineFailure::None.code()),
            inner: Mutex::new(SupervisorInner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SupervisorInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_state(&self, state: MobileEngineState) {
        self.state.store(state.code(), Ordering::Release);
        diagnostic("lifecycle", format_args!("state={}", state_name(state)));
    }

    fn fail(&self, failure: MobileEngineFailure) -> MobileEngineFailure {
        self.last_failure.store(failure.code(), Ordering::Release);
        self.state
            .store(MobileEngineState::Failed.code(), Ordering::Release);
        diagnostic(
            "lifecycle",
            format_args!(
                "state={} failure={} code={}",
                state_name(MobileEngineState::Failed),
                failure_name(failure),
                failure.code()
            ),
        );
        failure
    }

    fn state(&self) -> MobileEngineState {
        self.refresh_finished_worker();
        decode_state(self.state.load(Ordering::Acquire))
    }

    fn last_failure(&self) -> MobileEngineFailure {
        self.refresh_finished_worker();
        decode_failure(self.last_failure.load(Ordering::Acquire))
    }

    fn configure_storage(
        &self,
        inner: &mut SupervisorInner,
        requested: &Path,
    ) -> Result<PathBuf, MobileEngineFailure> {
        if !requested.is_absolute() {
            return Err(MobileEngineFailure::StorageConfiguration);
        }
        let configured = requested
            .canonicalize()
            .map_err(|_| MobileEngineFailure::StorageConfiguration)?;
        if !configured.is_dir() {
            return Err(MobileEngineFailure::StorageConfiguration);
        }
        match &inner.storage_directory {
            Some(existing) if existing != &configured => {
                Err(MobileEngineFailure::StorageConfiguration)
            }
            Some(existing) => Ok(existing.clone()),
            None => {
                diagnostic("storage", format_args!("configured=persistent"));
                inner.storage_directory = Some(configured.clone());
                Ok(configured)
            }
        }
    }

    fn reject_start_locked(
        &self,
        inner: &SupervisorInner,
        failure: MobileEngineFailure,
    ) -> MobileEngineFailure {
        self.last_failure.store(failure.code(), Ordering::Release);
        if inner
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.join.is_finished())
            && decode_state(self.state.load(Ordering::Acquire)) == MobileEngineState::Running
        {
            diagnostic(
                "lifecycle",
                format_args!(
                    "state=running rejected_start={} code={}",
                    failure_name(failure),
                    failure.code()
                ),
            );
            failure
        } else {
            self.fail(failure)
        }
    }

    fn reject_start(&self, failure: MobileEngineFailure) -> MobileEngineFailure {
        let mut inner = self.lock();
        self.reap_finished_locked(&mut inner);
        self.reject_start_locked(&inner, failure)
    }

    fn start(&self, requested: &Path) -> MobileEngineFailure {
        self.start_with_worker(requested, STARTUP_TIMEOUT, run_worker)
    }

    fn start_with_worker<F>(
        &self,
        requested: &Path,
        startup_timeout: Duration,
        worker_main: F,
    ) -> MobileEngineFailure
    where
        F: FnOnce(
                PathBuf,
                mpsc::SyncSender<Result<EngineRuntime, MobileEngineFailure>>,
                tokio::sync::oneshot::Receiver<()>,
            ) + Send
            + 'static,
    {
        let mut inner = self.lock();
        self.reap_finished_locked(&mut inner);
        let storage_directory = match self.configure_storage(&mut inner, requested) {
            Ok(path) => path,
            Err(failure) => return self.reject_start_locked(&inner, failure),
        };

        if let Some(worker) = inner.worker.as_ref() {
            if !worker.join.is_finished() {
                return match decode_state(self.state.load(Ordering::Acquire)) {
                    MobileEngineState::Running => {
                        self.last_failure
                            .store(MobileEngineFailure::None.code(), Ordering::Release);
                        diagnostic("lifecycle", format_args!("state=running idempotent=true"));
                        MobileEngineFailure::None
                    }
                    MobileEngineState::Starting => MobileEngineFailure::None,
                    MobileEngineState::Failed | MobileEngineState::Stopped => {
                        decode_failure(self.last_failure.load(Ordering::Acquire))
                    }
                };
            }
        }

        self.set_state(MobileEngineState::Starting);
        let (ready_tx, ready_rx) =
            mpsc::sync_channel::<Result<EngineRuntime, MobileEngineFailure>>(1);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let thread_storage = storage_directory.clone();
        let join = match thread::Builder::new()
            .name("hopspot-engine".into())
            .spawn(move || worker_main(thread_storage, ready_tx, shutdown_rx))
        {
            Ok(join) => join,
            Err(_) => return self.fail(MobileEngineFailure::WorkerSpawn),
        };
        inner.worker = Some(Worker {
            shutdown: Some(shutdown_tx),
            join,
        });

        match ready_rx.recv_timeout(startup_timeout) {
            Ok(Ok(runtime)) => {
                inner.runtime = Some(runtime);
                self.last_failure
                    .store(MobileEngineFailure::None.code(), Ordering::Release);
                self.set_state(MobileEngineState::Running);
                MobileEngineFailure::None
            }
            Ok(Err(failure)) => {
                self.wait_for_worker_locked(&mut inner, SHUTDOWN_TIMEOUT);
                self.fail(failure)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(worker) = inner.worker.as_mut() {
                    if let Some(shutdown) = worker.shutdown.take() {
                        let _ = shutdown.send(());
                    }
                }
                self.fail(MobileEngineFailure::StartupTimeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.reap_finished_locked(&mut inner);
                self.fail(MobileEngineFailure::WorkerStopped)
            }
        }
    }

    fn stop(&self) -> MobileEngineFailure {
        self.stop_with_timeout(SHUTDOWN_TIMEOUT)
    }

    fn stop_with_timeout(&self, shutdown_timeout: Duration) -> MobileEngineFailure {
        let mut inner = self.lock();
        self.reap_finished_locked(&mut inner);
        let Some(worker) = inner.worker.as_mut() else {
            inner.runtime = None;
            self.set_state(MobileEngineState::Stopped);
            diagnostic("shutdown", format_args!("complete=true idempotent=true"));
            return MobileEngineFailure::None;
        };
        if let Some(shutdown) = worker.shutdown.take() {
            let _ = shutdown.send(());
        }
        if !self.wait_for_worker_locked(&mut inner, shutdown_timeout) {
            return self.fail(MobileEngineFailure::ShutdownTimeout);
        }
        inner.runtime = None;
        self.set_state(MobileEngineState::Stopped);
        diagnostic("shutdown", format_args!("complete=true idempotent=false"));
        MobileEngineFailure::None
    }

    fn wait_for_worker_locked(&self, inner: &mut SupervisorInner, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while inner
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.join.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(SHUTDOWN_POLL);
        }
        if inner
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.join.is_finished())
        {
            return false;
        }
        self.take_finished_worker(inner);
        true
    }

    fn refresh_finished_worker(&self) {
        let mut inner = self.lock();
        self.reap_finished_locked(&mut inner);
    }

    fn reap_finished_locked(&self, inner: &mut SupervisorInner) {
        if !inner
            .worker
            .as_ref()
            .is_some_and(|worker| worker.join.is_finished())
        {
            return;
        }
        let was_running =
            decode_state(self.state.load(Ordering::Acquire)) == MobileEngineState::Running;
        self.take_finished_worker(inner);
        inner.runtime = None;
        if was_running {
            self.fail(MobileEngineFailure::WorkerStopped);
        }
    }

    fn take_finished_worker(&self, inner: &mut SupervisorInner) {
        if let Some(worker) = inner.worker.take() {
            let _ = worker.join.join();
        }
    }

    fn runtime(&self) -> Option<EngineRuntime> {
        self.refresh_finished_worker();
        self.lock().runtime.clone()
    }
}

static ENGINE: OnceLock<EngineSupervisor> = OnceLock::new();

fn supervisor() -> &'static EngineSupervisor {
    ENGINE.get_or_init(EngineSupervisor::new)
}

pub(crate) fn start(storage_directory: &Path) -> MobileEngineFailure {
    supervisor().start(storage_directory)
}

pub(crate) fn reject_storage_configuration() -> MobileEngineFailure {
    supervisor().reject_start(MobileEngineFailure::StorageConfiguration)
}

pub(crate) fn stop() -> MobileEngineFailure {
    supervisor().stop()
}

pub(crate) fn state() -> MobileEngineState {
    supervisor().state()
}

pub(crate) fn last_failure() -> MobileEngineFailure {
    supervisor().last_failure()
}

pub(crate) fn interface_snapshots() -> std::vec::Vec<InterfaceSnapshot> {
    supervisor()
        .runtime()
        .map_or_else(std::vec::Vec::new, |runtime| runtime.handle.interfaces())
}

pub(crate) fn toggle_interface(id: InterfaceId) {
    let Some(runtime) = supervisor().runtime() else {
        return;
    };
    if id == USB_INTERFACE_ID {
        runtime.usb_status.toggle_enabled();
    } else if id == runtime.wifi_status.id() {
        runtime.wifi_status.toggle_enabled();
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        if let Some(status) = &runtime.ble_status {
            status.toggle_enabled();
        }
    }
}

pub(crate) fn sleep_interfaces() {
    let Some(runtime) = supervisor().runtime() else {
        return;
    };
    runtime.usb_status.disable();
    runtime.wifi_status.disable();
    if let Some(status) = &runtime.ble_status {
        status.disable();
    }
}

pub(crate) fn wake_interfaces() {
    let Some(runtime) = supervisor().runtime() else {
        return;
    };
    runtime.usb_status.enable();
    runtime.wifi_status.enable();
    if let Some(status) = &runtime.ble_status {
        status.enable();
    }
}

pub(crate) fn announce() {
    let Some(runtime) = supervisor().runtime() else {
        return;
    };
    let _ = runtime.handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
        destination: runtime.node_page_destination,
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    }));
    diagnostic("announce", format_args!("destination=nomadnetwork.node"));
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, CardLabel)> {
    match id.kind() {
        Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("LAN"))),
        Some(InterfaceKind::UsbAutoDevice) => Some((CardKind::Usb, card_label("USB"))),
        Some(InterfaceKind::BluetoothAuto) => Some((CardKind::Ble, card_label("BLE"))),
        Some(InterfaceKind::TcpServerPeer | InterfaceKind::TcpClient | InterfaceKind::WifiPeer) => {
            Some(peer_card(id, CardKind::Peer, "LAN"))
        }
        Some(InterfaceKind::BluetoothPeer) => Some(peer_card(id, CardKind::Ble, "BLE")),
        _ => None,
    }
}

fn peer_card(id: InterfaceId, kind: CardKind, tag: &str) -> (CardKind, CardLabel) {
    let bytes = id.as_bytes();
    let mut label = CardLabel::new();
    let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
    (kind, label)
}

fn run_worker(
    storage_directory: PathBuf,
    ready_tx: mpsc::SyncSender<Result<EngineRuntime, MobileEngineFailure>>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = ready_tx.send(Err(MobileEngineFailure::RuntimeBuild));
            return;
        }
    };
    runtime.block_on(run_engine(storage_directory, ready_tx, shutdown_rx));
}

async fn run_engine(
    storage_directory: PathBuf,
    ready_tx: mpsc::SyncSender<Result<EngineRuntime, MobileEngineFailure>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let node_bootstrap =
        load_host_node_identity(&storage_directory.join(NODE_IDENTITY_STORAGE.as_str()));
    let node_identity = match require_persistent_identity("node", node_bootstrap) {
        Ok(identity) => identity,
        Err(failure) => {
            let _ = ready_tx.send(Err(failure));
            return;
        }
    };
    let ble_bootstrap =
        load_host_ble_identity(&storage_directory.join(BLE_IDENTITY_STORAGE.as_str()));
    let ble_identity = match require_persistent_identity("ble", ble_bootstrap) {
        Ok(identity) => identity,
        Err(failure) => {
            let _ = ready_tx.send(Err(failure));
            return;
        }
    };

    let node_public_identity =
        InMemoryNodeIdentity::from_secret_key_bytes(node_identity.secret()).identity_hash();
    diagnostic(
        "identity",
        format_args!(
            "node={} ble={}",
            abbreviated_hex(node_public_identity.as_bytes()),
            abbreviated_hex(ble_identity.as_bytes())
        ),
    );

    // CoreBluetooth's restoration-aware managers are created before unrelated transport work.
    // Readiness remains asynchronous inside the BLE supervisor, so denial or an unavailable radio
    // degrades that interface without delaying the core engine.
    let prepared_ble = match AutoBle::prepare(ble_identity).await {
        Ok(prepared) => prepared,
        Err(error) => {
            diagnostic(
                "transport",
                format_args!("kind=ble state=failed error={error:?}"),
            );
            AutoBle::unavailable(ble_identity)
        }
    };

    let usb_listener = match TcpListener::bind(("0.0.0.0", crate::usbmux::USBMUX_AUTO_PORT)).await {
        Ok(listener) => listener,
        Err(error) => {
            diagnostic(
                "listener",
                format_args!("kind=usbmux state=failed error={error}"),
            );
            let _ = ready_tx.send(Err(MobileEngineFailure::LocalListenerBind));
            return;
        }
    };
    let prepared_persistence = match PreparedPersistence::open(&storage_directory) {
        Ok(prepared) => prepared,
        Err(error) => {
            diagnostic(
                "persistence",
                format_args!("state=failed phase=prepare error={error}"),
            );
            let _ = ready_tx.send(Err(MobileEngineFailure::StorageConfiguration));
            return;
        }
    };

    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    let destinations = HopspotDestinationSet::new(
        destination_secret,
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    );
    let destination_hashes = match destinations.destination_hashes() {
        Ok(hashes) => hashes,
        Err(_) => {
            let _ = ready_tx.send(Err(MobileEngineFailure::StorageConfiguration));
            return;
        }
    };
    diagnostic(
        "destinations",
        format_args!(
            "delivery={} node_page={}",
            full_hex(destination_hashes.delivery.as_bytes()),
            full_hex(destination_hashes.node_page.as_bytes())
        ),
    );

    let (persistence_change_tx, persistence_changes) = tokio::sync::mpsc::unbounded_channel::<()>();
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel::<DestinationHash>();
    let timeline_origin = prepared_persistence.timeline_origin();
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_hopspot_core::node_pages::NodePageRoutes,
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event: PrnsEvent<'_>, _state: &()| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
                app_data: _,
            }) => {
                diagnostic(
                    "route",
                    format_args!(
                        "state=accepted destination={} hops={} interface={}",
                        full_hex(destination.as_bytes()),
                        hops,
                        abbreviated_hex(source_interface.as_bytes())
                    ),
                );
                let _ = persistence_change_tx.send(());
            }
            PrnsEvent::Diagnostic(Diagnostic::RouteRemoved { destination, cause }) => {
                diagnostic(
                    "route",
                    format_args!(
                        "state=removed destination={} cause={cause:?}",
                        full_hex(destination.as_bytes())
                    ),
                );
                let _ = persistence_change_tx.send(());
            }
            PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) => {
                let _ = rotated_tx.send(destination);
            }
            _ => {}
        },
    })
    .with_timeline_origin(timeline_origin);
    let restored = prepared_persistence.restore(&mut node);
    diagnostic(
        "persistence",
        format_args!(
            "state=restored routes={} destinations={} tunnels={} ratchets={} refused={} dropped={}",
            restored.routes.seeded_count,
            restored.destination_identities.seeded_count,
            restored.tunnels.seeded_count,
            restored.ratchets.seeded_count,
            restored.refused_total(),
            restored.dropped_total()
        ),
    );
    let handle = node.handle();

    let usb = crate::usbmux::UsbMuxAutoDevice::with_listener(USB_INTERFACE_ID, usb_listener);
    let usb_status = usb.status();
    handle.add_interface(usb);
    diagnostic(
        "listener",
        format_args!(
            "kind=usbmux state=bound port={}",
            crate::usbmux::USBMUX_AUTO_PORT
        ),
    );

    #[cfg(target_os = "ios")]
    let wifi = AutoWifi::new().with_platform_discovery(apple_service_discovery());
    #[cfg(not(target_os = "ios"))]
    let wifi = AutoWifi::new();
    let wifi_status = wifi.status();
    handle.supervise(wifi);
    diagnostic(
        "listener",
        format_args!(
            "kind=wifi state=managed port={}",
            wifi_auto_contract::TCP_RENDEZVOUS_PORT
        ),
    );

    let attached_ble = handle.attach(prepared_ble);
    let ble_id = attached_ble.id();
    let ble_status = Some(attached_ble.status());
    diagnostic("transport", format_args!("kind=ble state=prepared"));

    let engine_runtime = EngineRuntime {
        handle: handle.clone(),
        usb_status,
        wifi_status: wifi_status.clone(),
        ble_status,
        node_page_destination: destination_hashes.node_page,
    };
    let mut persistence_task =
        prepared_persistence.start(handle.clone(), persistence_changes, rotated_rx);
    diagnostic(
        "memory",
        format_args!("resident_bytes={}", resident_bytes()),
    );
    if ready_tx.send(Ok(engine_runtime)).is_err() {
        persistence_task.abort().await;
        return;
    }

    let diagnostics = tokio::spawn(log_runtime_diagnostics(handle.clone()));
    let mut node_run = Box::pin(node.run());
    let unexpected = tokio::select! {
        result = &mut node_run => {
            log_unexpected_worker_exit(result);
            true
        }
        _ = &mut shutdown_rx => {
            diagnostic("shutdown", format_args!("requested=true"));
            let mut stopped_during_flush = false;
            let persistence_shutdown = tokio::select! {
                outcome = persistence_task.shutdown(PERSISTENCE_SHUTDOWN_TIMEOUT) => outcome,
                result = &mut node_run => {
                    log_unexpected_worker_exit(result);
                    stopped_during_flush = true;
                    PersistenceShutdown::Failed
                }
            };
            diagnostic(
                "persistence",
                format_args!(
                    "state=shutdown result={}",
                    persistence_shutdown_name(persistence_shutdown)
                ),
            );
            if !stopped_during_flush {
                handle.remove_interface(USB_INTERFACE_ID);
                handle.remove_interface(wifi_status.id());
                handle.remove_interface(ble_id);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            stopped_during_flush
        }
    };
    persistence_task.abort().await;
    diagnostics.abort();
    let _ = diagnostics.await;
    drop(node_run);
    diagnostic(
        "worker",
        format_args!(
            "exit={} resources_dropped=true",
            if unexpected {
                "unexpected"
            } else {
                "requested"
            }
        ),
    );
}

fn log_unexpected_worker_exit(result: Result<(), NodeRunError>) {
    match result {
        Ok(()) => diagnostic("worker", format_args!("exit=unexpected result=ok")),
        Err(error) => diagnostic("worker", format_args!("exit=unexpected error={error}")),
    }
}

fn require_persistent_identity<I, E: core::fmt::Display>(
    label: &str,
    bootstrap: IdentityBootstrap<I, E>,
) -> Result<I, MobileEngineFailure> {
    match bootstrap.persistence() {
        IdentityPersistence::Loaded => {
            diagnostic(
                "storage",
                format_args!("identity={label} persistence=loaded"),
            );
        }
        IdentityPersistence::Created => {
            diagnostic(
                "storage",
                format_args!("identity={label} persistence=created"),
            );
        }
        IdentityPersistence::Recovered(error) => {
            diagnostic(
                "storage",
                format_args!("identity={label} persistence=recovered error={error}"),
            );
        }
        IdentityPersistence::Ephemeral(error) => {
            diagnostic(
                "storage",
                format_args!("identity={label} persistence=ephemeral error={error}"),
            );
            return Err(MobileEngineFailure::StorageConfiguration);
        }
    }
    Ok(bootstrap.into_identity())
}

async fn log_runtime_diagnostics(handle: PrnsNodeHandle) {
    let mut interval = tokio::time::interval(DIAGNOSTIC_INTERVAL);
    let mut samples = 0_u64;
    loop {
        interval.tick().await;
        samples = samples.saturating_add(1);
        for snapshot in handle.interfaces() {
            diagnostic(
                "transport",
                format_args!(
                    "id={} state={:?} rx={} tx={}",
                    abbreviated_hex(snapshot.id.as_bytes()),
                    snapshot.connection,
                    snapshot.rx_bytes,
                    snapshot.tx_bytes
                ),
            );
        }
        diagnostic(
            "routes",
            format_args!("count={}", handle.routes().await.len()),
        );
        if samples.is_multiple_of(MEMORY_DIAGNOSTIC_INTERVALS) {
            diagnostic(
                "memory",
                format_args!("resident_bytes={}", resident_bytes()),
            );
        }
    }
}

pub(crate) fn diagnostic(kind: &str, fields: core::fmt::Arguments<'_>) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    println!("HOPSPOT_IOS ts_ms={millis} kind={kind} {fields}");
}

const fn persistence_shutdown_name(outcome: PersistenceShutdown) -> &'static str {
    match outcome {
        PersistenceShutdown::Flushed => "flushed",
        PersistenceShutdown::Failed => "failed",
        PersistenceShutdown::TimedOut => "timed_out",
        PersistenceShutdown::AlreadyStopped => "already_stopped",
    }
}

fn abbreviated_hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes.iter().take(4) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn full_hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` points at a correctly sized writable rusage object, and getrusage
    // initializes it on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result == 0 {
        // SAFETY: getrusage succeeded and initialized the value.
        unsafe { usage.assume_init() }.ru_maxrss as u64
    } else {
        0
    }
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn resident_bytes() -> u64 {
    0
}

const fn state_name(state: MobileEngineState) -> &'static str {
    match state {
        MobileEngineState::Stopped => "stopped",
        MobileEngineState::Starting => "starting",
        MobileEngineState::Running => "running",
        MobileEngineState::Failed => "failed",
    }
}

const fn failure_name(failure: MobileEngineFailure) -> &'static str {
    match failure {
        MobileEngineFailure::None => "none",
        MobileEngineFailure::StorageConfiguration => "storage_configuration",
        MobileEngineFailure::WorkerSpawn => "worker_spawn",
        MobileEngineFailure::RuntimeBuild => "runtime_build",
        MobileEngineFailure::LocalListenerBind => "local_listener_bind",
        MobileEngineFailure::RpcListenerBind => "rpc_listener_bind",
        MobileEngineFailure::StartupTimeout => "startup_timeout",
        MobileEngineFailure::WorkerStopped => "worker_stopped",
        MobileEngineFailure::ShutdownTimeout => "shutdown_timeout",
        MobileEngineFailure::PersistenceWrite => "persistence_write",
    }
}

const fn decode_state(code: i32) -> MobileEngineState {
    match code {
        1 => MobileEngineState::Starting,
        2 => MobileEngineState::Running,
        3 => MobileEngineState::Failed,
        _ => MobileEngineState::Stopped,
    }
}

const fn decode_failure(code: i32) -> MobileEngineFailure {
    match code {
        1 => MobileEngineFailure::StorageConfiguration,
        2 => MobileEngineFailure::WorkerSpawn,
        3 => MobileEngineFailure::RuntimeBuild,
        4 => MobileEngineFailure::LocalListenerBind,
        5 => MobileEngineFailure::RpcListenerBind,
        6 => MobileEngineFailure::StartupTimeout,
        7 => MobileEngineFailure::WorkerStopped,
        8 => MobileEngineFailure::ShutdownTimeout,
        9 => MobileEngineFailure::PersistenceWrite,
        _ => MobileEngineFailure::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ios_interface_label_fits_its_card() {
        let kinds = [
            InterfaceKind::AutoWifi,
            InterfaceKind::UsbAutoDevice,
            InterfaceKind::BluetoothAuto,
            InterfaceKind::TcpServerPeer,
            InterfaceKind::TcpClient,
            InterfaceKind::WifiPeer,
            InterfaceKind::BluetoothPeer,
        ];
        for kind in kinds {
            let id = InterfaceId::from_channel_tag(kind, b"label-fit");
            let (card_kind, label) = classify(id).expect("known interface has a card");
            assert!(
                label.chars().count() <= personal_hopspot_core::card_label_max_chars(card_kind),
                "{kind:?} label {label:?} exceeds its card"
            );
        }
    }
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn lifecycle_and_failure_codes_are_stable_and_closed() {
        assert_eq!(MobileEngineState::Stopped.code(), 0);
        assert_eq!(MobileEngineState::Starting.code(), 1);
        assert_eq!(MobileEngineState::Running.code(), 2);
        assert_eq!(MobileEngineState::Failed.code(), 3);
        let failures = [
            MobileEngineFailure::None,
            MobileEngineFailure::StorageConfiguration,
            MobileEngineFailure::WorkerSpawn,
            MobileEngineFailure::RuntimeBuild,
            MobileEngineFailure::LocalListenerBind,
            MobileEngineFailure::RpcListenerBind,
            MobileEngineFailure::StartupTimeout,
            MobileEngineFailure::WorkerStopped,
            MobileEngineFailure::ShutdownTimeout,
            MobileEngineFailure::PersistenceWrite,
        ];
        for (code, failure) in failures.into_iter().enumerate() {
            assert_eq!(failure.code(), code as i32);
            assert_eq!(decode_failure(code as i32), failure);
        }
    }

    #[test]
    fn first_storage_path_is_permanently_bound_and_conflicts_are_rejected() {
        let first = tempfile::tempdir().unwrap();
        let conflicting = tempfile::tempdir().unwrap();
        let supervisor = EngineSupervisor::new();
        let mut inner = supervisor.lock();
        let configured = supervisor
            .configure_storage(&mut inner, first.path())
            .unwrap();
        assert_eq!(
            supervisor.configure_storage(&mut inner, first.path()),
            Ok(configured)
        );
        assert_eq!(
            supervisor.configure_storage(&mut inner, conflicting.path()),
            Err(MobileEngineFailure::StorageConfiguration)
        );
        assert_eq!(
            supervisor.configure_storage(&mut inner, Path::new("relative")),
            Err(MobileEngineFailure::StorageConfiguration)
        );
    }

    #[test]
    fn identity_persistence_rejects_ephemeral_and_accepts_durable_variants() {
        assert_eq!(
            require_persistent_identity(
                "test",
                IdentityBootstrap::<u8, &str>::ephemeral(7, "write failed")
            ),
            Err(MobileEngineFailure::StorageConfiguration)
        );
        assert_eq!(
            require_persistent_identity("test", IdentityBootstrap::<u8, &str>::loaded(7)),
            Ok(7)
        );
        assert_eq!(
            require_persistent_identity("test", IdentityBootstrap::<u8, &str>::created(8)),
            Ok(8)
        );
        assert_eq!(
            require_persistent_identity(
                "test",
                IdentityBootstrap::<u8, &str>::recovered(9, "corrupt")
            ),
            Ok(9)
        );
    }

    #[test]
    fn every_fault_maps_to_the_required_failed_state() {
        for failure in [
            MobileEngineFailure::StorageConfiguration,
            MobileEngineFailure::WorkerSpawn,
            MobileEngineFailure::RuntimeBuild,
            MobileEngineFailure::LocalListenerBind,
            MobileEngineFailure::RpcListenerBind,
            MobileEngineFailure::StartupTimeout,
            MobileEngineFailure::WorkerStopped,
            MobileEngineFailure::ShutdownTimeout,
            MobileEngineFailure::PersistenceWrite,
        ] {
            let supervisor = EngineSupervisor::new();
            assert_eq!(supervisor.fail(failure), failure);
            assert_eq!(supervisor.state(), MobileEngineState::Failed);
            assert_eq!(supervisor.last_failure(), failure);
        }
    }

    #[test]
    fn successful_start_is_the_only_operation_that_clears_a_failure() {
        let supervisor = EngineSupervisor::new();
        supervisor.fail(MobileEngineFailure::StartupTimeout);
        supervisor.set_state(MobileEngineState::Stopped);
        assert_eq!(
            supervisor.last_failure(),
            MobileEngineFailure::StartupTimeout
        );
        supervisor
            .last_failure
            .store(MobileEngineFailure::None.code(), Ordering::Release);
        supervisor.set_state(MobileEngineState::Running);
        assert_eq!(supervisor.last_failure(), MobileEngineFailure::None);
    }

    #[test]
    fn same_path_start_and_stopped_stop_are_idempotent() {
        let storage = tempfile::tempdir().unwrap();
        let conflicting = tempfile::tempdir().unwrap();
        let supervisor = EngineSupervisor::new();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let join = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        {
            let mut inner = supervisor.lock();
            supervisor
                .configure_storage(&mut inner, storage.path())
                .unwrap();
            let (shutdown, _shutdown_rx) = tokio::sync::oneshot::channel();
            inner.worker = Some(Worker {
                shutdown: Some(shutdown),
                join,
            });
        }
        supervisor.set_state(MobileEngineState::Running);

        let conflicting_replacement_started = Arc::new(AtomicBool::new(false));
        let conflicting_replacement_flag = conflicting_replacement_started.clone();
        assert_eq!(
            supervisor.start_with_worker(
                conflicting.path(),
                Duration::from_millis(20),
                move |_, _, _| conflicting_replacement_flag.store(true, Ordering::Release)
            ),
            MobileEngineFailure::StorageConfiguration
        );
        assert_eq!(supervisor.state(), MobileEngineState::Running);
        assert_eq!(
            supervisor.last_failure(),
            MobileEngineFailure::StorageConfiguration
        );
        assert!(!conflicting_replacement_started.load(Ordering::Acquire));

        let replacement_started = Arc::new(AtomicBool::new(false));
        let replacement_flag = replacement_started.clone();
        assert_eq!(
            supervisor.start_with_worker(
                storage.path(),
                Duration::from_millis(20),
                move |_, _, _| replacement_flag.store(true, Ordering::Release)
            ),
            MobileEngineFailure::None
        );
        assert_eq!(supervisor.state(), MobileEngineState::Running);
        assert_eq!(supervisor.last_failure(), MobileEngineFailure::None);
        assert!(!replacement_started.load(Ordering::Acquire));

        release_tx.send(()).unwrap();
        wait_for_test_worker(&supervisor);
        assert_eq!(supervisor.state(), MobileEngineState::Failed);
        assert_eq!(
            supervisor.last_failure(),
            MobileEngineFailure::WorkerStopped
        );

        let stopped = EngineSupervisor::new();
        assert_eq!(
            stopped.stop_with_timeout(Duration::from_millis(20)),
            MobileEngineFailure::None
        );
        assert_eq!(stopped.state(), MobileEngineState::Stopped);
    }

    #[test]
    fn startup_timeout_retains_worker_and_refuses_replacement_until_exit() {
        let storage = tempfile::tempdir().unwrap();
        let supervisor = EngineSupervisor::new();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        assert_eq!(
            supervisor.start_with_worker(
                storage.path(),
                Duration::from_millis(20),
                move |_, _, _| {
                    let _ = release_rx.recv();
                }
            ),
            MobileEngineFailure::StartupTimeout
        );
        assert_eq!(supervisor.state(), MobileEngineState::Failed);
        assert!(!supervisor
            .lock()
            .worker
            .as_ref()
            .unwrap()
            .join
            .is_finished());

        let replacement_started = Arc::new(AtomicBool::new(false));
        let replacement_flag = replacement_started.clone();
        assert_eq!(
            supervisor.start_with_worker(
                storage.path(),
                Duration::from_millis(20),
                move |_, _, _| replacement_flag.store(true, Ordering::Release)
            ),
            MobileEngineFailure::StartupTimeout
        );
        assert!(!replacement_started.load(Ordering::Acquire));

        release_tx.send(()).unwrap();
        wait_for_test_worker(&supervisor);
        assert_eq!(supervisor.state(), MobileEngineState::Failed);

        assert_eq!(
            supervisor.start_with_worker(storage.path(), Duration::from_secs(1), |_, ready, _| {
                let _ = ready.send(Err(MobileEngineFailure::RuntimeBuild));
            }),
            MobileEngineFailure::RuntimeBuild
        );
    }

    #[test]
    fn shutdown_timeout_retains_worker_until_it_can_be_joined() {
        let supervisor = EngineSupervisor::new();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let join = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        {
            let mut inner = supervisor.lock();
            let (shutdown, _shutdown_rx) = tokio::sync::oneshot::channel();
            inner.worker = Some(Worker {
                shutdown: Some(shutdown),
                join,
            });
        }
        supervisor.set_state(MobileEngineState::Running);

        assert_eq!(
            supervisor.stop_with_timeout(Duration::from_millis(20)),
            MobileEngineFailure::ShutdownTimeout
        );
        assert!(supervisor.lock().worker.is_some());
        assert_eq!(supervisor.state(), MobileEngineState::Failed);

        release_tx.send(()).unwrap();
        wait_for_test_worker(&supervisor);
        assert_eq!(supervisor.state(), MobileEngineState::Failed);
        assert!(supervisor.lock().worker.is_none());
        assert_eq!(
            supervisor.last_failure(),
            MobileEngineFailure::ShutdownTimeout
        );
    }

    #[test]
    fn unexpected_worker_exit_is_reported() {
        let supervisor = EngineSupervisor::new();
        let join = thread::spawn(|| {});
        {
            let mut inner = supervisor.lock();
            let (shutdown, _shutdown_rx) = tokio::sync::oneshot::channel();
            inner.worker = Some(Worker {
                shutdown: Some(shutdown),
                join,
            });
        }
        supervisor.set_state(MobileEngineState::Running);
        wait_for_test_worker(&supervisor);

        assert_eq!(supervisor.state(), MobileEngineState::Failed);
        assert_eq!(
            supervisor.last_failure(),
            MobileEngineFailure::WorkerStopped
        );
        assert!(supervisor.lock().worker.is_none());
    }

    #[test]
    fn restart_reloads_stable_identities_and_destinations() {
        fn load_evidence(directory: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
            let node = require_persistent_identity(
                "node",
                load_host_node_identity(&directory.join(NODE_IDENTITY_STORAGE.as_str())),
            )
            .unwrap();
            let ble = require_persistent_identity(
                "ble",
                load_host_ble_identity(&directory.join(BLE_IDENTITY_STORAGE.as_str())),
            )
            .unwrap();
            let node_identity = node.secret().to_vec();
            let ble_identity = ble.as_bytes().to_vec();
            let destinations = HopspotDestinationSet::new(
                node.into_destination_secret(),
                ANNOUNCE_APP_DATA,
                NODE_ANNOUNCE_APP_DATA,
            )
            .destination_hashes()
            .unwrap();
            (
                node_identity,
                ble_identity,
                destinations.delivery.as_bytes().to_vec(),
                destinations.node_page.as_bytes().to_vec(),
            )
        }

        let storage = tempfile::tempdir().unwrap();
        let first = load_evidence(storage.path());
        let restarted = load_evidence(storage.path());
        assert_eq!(first, restarted);
    }

    fn wait_for_test_worker(supervisor: &EngineSupervisor) {
        while !supervisor
            .lock()
            .worker
            .as_ref()
            .unwrap()
            .join
            .is_finished()
        {
            thread::yield_now();
        }
    }
}
