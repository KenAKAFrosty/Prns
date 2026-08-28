use personal_rns::runtime::NoPersistence;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use personal_hopspot_core::{
    load_host_ble_identity, load_host_node_identity, HopspotDestinationSet, IdentityPersistence,
    MobileEngineFailure, BLE_IDENTITY_STORAGE, NODE_IDENTITY_STORAGE,
};
use personal_rns::bluetooth_auto::BluetoothAuto;
use personal_rns::interfaces::bluetooth_auto::{
    AndroidHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::interfaces::wifi_direct::GoIntent;
use personal_rns::runtime::{Diagnostic, ManuallyAttached, PrnsEvent, PrnsNode, PrnsNodeRecipe};
use personal_rns::shared_instance::rns_rpc::{SharedInstanceCredentials, SharedInstanceRpcServer};
use personal_rns::shared_instance::SharedInstanceServer;
use personal_rns::storage::GrowableHeap;
use personal_rns::usb_auto::{UsbAutoCandidate, UsbAutoHost};
use personal_rns::wifi_auto::AutoWifi;
use personal_rns::wifi_aware::WifiAwareAuto;
use personal_rns::wifi_direct::WifiDirectAuto;
use tokio::sync::oneshot;

use super::{
    persistence::{self, PersistenceRestore, PersistenceWorker},
    EnginePorts, EngineResources, EngineStartError, PlatformLinks, WorkerExit, ANDROID_PORT,
    ANNOUNCE_APP_DATA, NODE_ANNOUNCE_APP_DATA, USB_INTERFACE_ID, WORKER_SHUTDOWN_TIMEOUT,
};
use crate::bluetooth_auto::AndroidBleBackend;
use crate::bridge::BridgeStream;
use crate::wifi_aware::AndroidWifiAwareBackend;
use crate::wifi_direct::AndroidWifiDirectBackend;

pub(super) struct WorkerInput {
    pub(super) storage_dir: PathBuf,
    pub(super) platform: PlatformLinks,
    pub(super) ready_tx: Sender<Result<EngineResources, EngineStartError>>,
    pub(super) shutdown_rx: oneshot::Receiver<()>,
    pub(super) stopped_tx: Sender<WorkerExit>,
    pub(super) ports: EnginePorts,
}

pub(super) fn run(input: WorkerInput) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let error = EngineStartError::RuntimeBuild;
            let _ = input.ready_tx.send(Err(error));
            let _ = input.stopped_tx.send(WorkerExit::Failed(error.failure()));
            return;
        }
    };
    let stopped_tx = input.stopped_tx.clone();
    let exit = runtime.block_on(run_engine(input));
    runtime.shutdown_timeout(WORKER_SHUTDOWN_TIMEOUT);
    let _ = stopped_tx.send(exit);
}

async fn run_engine(input: WorkerInput) -> WorkerExit {
    let WorkerInput {
        storage_dir,
        platform,
        ready_tx,
        shutdown_rx,
        ports,
        ..
    } = input;

    let node_bootstrap = load_host_node_identity(&storage_dir.join(NODE_IDENTITY_STORAGE.as_str()));
    if log_identity_persistence("node", node_bootstrap.persistence()).is_err() {
        return fail_start(ready_tx, EngineStartError::StorageConfiguration);
    }
    let node_identity = node_bootstrap.into_identity();
    let credentials = SharedInstanceCredentials::from_identity_secret(node_identity.secret());
    let node_identity_hash = credentials.transport_identity_hash();
    let rpc_key = credentials.rpc_key().clone();
    let transport_secret = node_identity.transport_secret();

    let ble_bootstrap = load_host_ble_identity(&storage_dir.join(BLE_IDENTITY_STORAGE.as_str()));
    if log_identity_persistence("Bluetooth", ble_bootstrap.persistence()).is_err() {
        return fail_start(ready_tx, EngineStartError::StorageConfiguration);
    }
    let ble_identity = ble_bootstrap.into_identity();
    platform.ble.set_local_identity(ble_identity);

    let destinations = HopspotDestinationSet::new(
        node_identity.into_destination_secret(),
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    );
    let destination_hashes = destinations
        .destination_hashes()
        .expect("the hopspot destination names are valid");

    let persistence_store = match persistence::open(&storage_dir) {
        Ok(persistence_store) => persistence_store,
        Err(error) => {
            log::error!("hopspot persistence storage unavailable: {error}");
            return fail_start(ready_tx, EngineStartError::StorageConfiguration);
        }
    };
    let timeline_origin = persistence_store.timeline_origin();
    let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_hopspot_core::node_pages::NodePageRoutes,
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) = event {
                let _ = rotated_tx.send(destination);
            }
        },
    })
    .with_timeline_origin(timeline_origin);
    let restored = persistence_store.restore(&mut node);
    let restore = PersistenceRestore::from_report(&restored);
    let handle = node.handle();
    let persistence =
        PersistenceWorker::new(handle.clone(), persistence_store, rotated_rx, restore);
    let persistence_health = persistence.health();

    let scan = {
        let bridge = platform.usb.clone();
        move || {
            if bridge.is_connected() {
                vec![UsbAutoCandidate::prns_specific(ANDROID_PORT)]
            } else {
                Vec::new()
            }
        }
    };
    let open = {
        let bridge = platform.usb.clone();
        move |_candidate: UsbAutoCandidate| {
            let bridge = bridge.clone();
            async move { Ok::<BridgeStream, io::Error>(bridge.open_stream()) }
        }
    };
    let usb = UsbAutoHost::new(USB_INTERFACE_ID, scan, open, platform.usb.rescan());
    let usb_status = usb.status();
    handle.add_interface(usb);

    let local = match SharedInstanceServer::with_port(ports.local).bind().await {
        Ok(local) => local,
        Err(_) => return fail_start(ready_tx, EngineStartError::LocalListenerBind),
    };
    handle.supervise(local);

    let rpc = match SharedInstanceRpcServer::tcp(credentials, ports.rpc, handle.clone())
        .bind()
        .await
    {
        Ok(rpc) => rpc,
        Err(_) => return fail_start(ready_tx, EngineStartError::RpcListenerBind),
    };
    tokio::spawn(rpc.run());

    let service_discovery = match platform.service_discovery.take_service_discovery() {
        Ok(service_discovery) => service_discovery,
        Err(_service_discovery_unavailable) => {
            return fail_start(ready_tx, EngineStartError::WorkerStopped);
        }
    };
    let wifi = AutoWifi::new().with_platform_discovery(service_discovery);
    let wifi_status = wifi.status();
    handle.supervise(wifi);

    let bluetooth = BluetoothAuto::<_, { AndroidBleBackend::MAX_PEERS }>::new(
        AndroidBleBackend::new(platform.ble),
        ble_identity,
        Endpoint::Android(AndroidHost::Android),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    );
    let ble_status = bluetooth.status();
    handle.supervise(bluetooth);

    let wifi_direct = WifiDirectAuto::new(
        AndroidWifiDirectBackend::new(platform.wifi_direct),
        GoIntent::PREFER_OWNER,
    );
    let wd_status = wifi_direct.status();
    handle.supervise(wifi_direct);

    let wifi_aware = WifiAwareAuto::new(AndroidWifiAwareBackend::new(platform.wifi_aware));
    let wa_status = wifi_aware.status();
    handle.supervise(wifi_aware);

    let resources = EngineResources {
        started_at: Instant::now(),
        node_identity_hash,
        ble_identity,
        usb_status,
        wifi_status,
        ble_status,
        wd_status,
        wa_status,
        handle: handle.clone(),
        destination: destination_hashes.delivery,
        node_page_destination: destination_hashes.node_page,
        rpc_key,
        ports,
        persistence: persistence_health,
    };
    let mut node_run = Box::pin(node.run());
    let initialized = {
        let initialization = persistence.initialize();
        tokio::pin!(initialization);
        tokio::select! {
            result = &mut node_run => {
                match result {
                    Ok(()) => {
                        log::error!("hopspot engine stopped during persistence initialization")
                    }
                    Err(error) => {
                        log::error!(
                            "hopspot engine stopped during persistence initialization: {error}"
                        )
                    }
                }
                return fail_start(ready_tx, EngineStartError::WorkerStopped);
            }
            result = &mut initialization => result,
        }
    };
    if initialized.is_err() {
        return fail_start(ready_tx, EngineStartError::PersistenceWrite);
    }
    if ready_tx.send(Ok(resources)).is_err() {
        return WorkerExit::Stopped;
    }

    let persistence_run = persistence.run(shutdown_rx);
    tokio::pin!(persistence_run);
    tokio::select! {
        result = &mut node_run => {
            match result {
                Ok(()) => log::error!("hopspot engine stopped"),
                Err(error) => log::error!("hopspot engine stopped: {error}"),
            }
            WorkerExit::Failed(MobileEngineFailure::WorkerStopped)
        }
        result = &mut persistence_run => {
            match result {
                Ok(()) => WorkerExit::Stopped,
                Err(()) => WorkerExit::Failed(MobileEngineFailure::PersistenceWrite),
            }
        }
    }
}

fn fail_start(
    ready_tx: Sender<Result<EngineResources, EngineStartError>>,
    error: EngineStartError,
) -> WorkerExit {
    let _ = ready_tx.send(Err(error));
    WorkerExit::Failed(error.failure())
}

fn log_identity_persistence(
    identity: &str,
    persistence: &IdentityPersistence<impl core::fmt::Display>,
) -> Result<(), ()> {
    match persistence {
        IdentityPersistence::Loaded => Ok(()),
        IdentityPersistence::Created => {
            log::info!("{identity} identity created");
            Ok(())
        }
        IdentityPersistence::Recovered(error) => {
            log::warn!("{identity} identity recovered after corruption: {error}");
            Ok(())
        }
        IdentityPersistence::Ephemeral(error) => {
            log::error!("{identity} identity is ephemeral: {error}");
            Err(())
        }
    }
}
