use super::*;
use personal_hopspot_core::S3_8_MIB_FLASH_LAYOUT;
use personal_rns::interfaces::lora::AirtimePolicy;
use personal_rns::lora::{LoRaInterfaceInput, LoRaSpectrumStatus};
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlSelfAnnouncement, RemoteControlService,
};
use personal_rns::runtime::{PrnsNodeHandle, PrnsNodeRecipe, SharedNorFlash};

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x27Personal Hopspot Wireless Stick Lite V3\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot Wireless Stick Lite V3";

pub async fn run(spawner: Spawner) {
    let S3Fn8Hardware {
        usb_rx,
        usb_tx,
        lora_radio,
        bluetooth,
        identity_entropy,
        mac,
        timebase,
        _rtc,
        _vext,
        _adc_control,
    } = board::bringup();

    let mut boot_entropy = entropy::seed_runtime_entropy(&identity_entropy)
        .expect("the enabled S3 boot TRNG fills the initial seed");
    let node_bootstrap = crate::identity::bootstrap_node_identity(&mut boot_entropy);
    crate::identity::log_persistence("node", node_bootstrap.persistence());
    let remote_control_bootstrap =
        crate::identity::RemoteControlIdentityFlash::from(S3_8_MIB_FLASH_LAYOUT)
            .load_or_generate_with_runtime_entropy(&mut boot_entropy)
            .expect("RemoteControl identity bootstrap failed");
    let ble_bootstrap = crate::identity::bootstrap_ble_identity(&mut boot_entropy);
    crate::identity::log_persistence("Bluetooth", ble_bootstrap.persistence());
    drop(identity_entropy);
    entropy::install(boot_entropy);
    let runtime_entropy = entropy::runtime_entropy();

    static FLASH: StaticCell<Mutex<Mtx, crate::flash::EspRomFlash>> = StaticCell::new();
    let flash = FLASH.init(Mutex::new(crate::flash::EspRomFlash::new(
        S3_8_MIB_FLASH_LAYOUT.flash_capacity,
    )));
    let shared_flash = SharedNorFlash::new(flash, S3_8_MIB_FLASH_LAYOUT.flash_capacity);
    let mut profile_store = personal_hopspot_core::RadioProfileStore::new(
        shared_flash,
        S3_8_MIB_FLASH_LAYOUT.radio_profile_pages,
    );
    let loaded_profile = match profile_store.load(US915_AUTO_LORA_PROFILE).await {
        Ok(loaded) => loaded,
        Err(error) => {
            log::error!("LoRa profile restore failed: {error:?}");
            personal_hopspot_core::LoadedRadioProfile {
                profile: US915_AUTO_LORA_PROFILE,
                follows_default: true,
                notice: Some(personal_hopspot_core::RadioProfileLoadNotice::Reset),
            }
        }
    };
    let lora_profile = loaded_profile.profile;
    let lora_id = LoRaInterface::<LoraRadio>::interface_id(&lora_profile);
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new_accounted(lora_id, ConnectionState::Initializing),
    );
    static LORA_SPECTRUM: StaticCell<LoRaSpectrumStatus> = StaticCell::new();
    let lora_spectrum: &'static LoRaSpectrumStatus = LORA_SPECTRUM.init(LoRaSpectrumStatus::new());
    static LORA_TX_QUEUE: StaticCell<[u8; personal_rns::lora::LORA_TX_QUEUE_BYTES]> =
        StaticCell::new();
    let lora_tx_queue: &'static mut [u8; personal_rns::lora::LORA_TX_QUEUE_BYTES] =
        LORA_TX_QUEUE.init([0; personal_rns::lora::LORA_TX_QUEUE_BYTES]);
    let lora = match LoRaInterface::new(LoRaInterfaceInput {
        radio: lora_radio,
        profile: lora_profile,
        airtime_policy: AirtimePolicy::Regional,
        tx_queue: lora_tx_queue,
        control: &LORA_CONTROL,
        status: lora_status,
        spectrum: lora_spectrum,
        lifecycle: LIFECYCLE.dyn_sender(),
    }) {
        Ok(lora) => lora,
        Err(_) => panic!("the built-in LoRa profile and regional policy are valid"),
    };

    let node_identity = node_bootstrap.into_identity();
    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    let destinations = personal_hopspot_core::HopspotDestinationSet::new(
        destination_secret,
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    );
    let node_page_destination = destinations
        .destination_hashes()
        .expect("the hopspot destination names are valid")
        .node_page;
    let (remote_control_identity_secrets, _remote_control_identity_origins) =
        remote_control_bootstrap.into_parts();
    let remote_control = RemoteControlService::new(
        remote_control_identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Destination(node_page_destination),
    );
    let ble_identity = ble_bootstrap.into_identity();

    let mut manifold_lanes = ManifoldLanes::new();
    let usb_lane = manifold_lanes
        .claim_accounted_interface(
            &USB_MANIFOLD_LANE,
            device_descriptor(USB_INTERFACE_ID, USB_UART_PAYLOAD_BITRATE_BPS),
            &USB_STATUS,
        )
        .expect("USB lane is available");
    let lora_lane = manifold_lanes
        .claim_accounted_interface(&LORA_MANIFOLD_LANE, lora.descriptor(), lora_status)
        .expect("LoRa lane is available");
    let ble_lane = manifold_lanes
        .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &BLE_OUTBOUND_WAKE)
        .expect("Bluetooth supervisor lane is available");

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );

    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    let ble_fleet: BleFleet = ble_lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
    let host = EmbassyHost::new_with_timebase(timebase, runtime_entropy);
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        remote_control,
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: (),
        storage: InternalStorage,
        request_endpoints: personal_hopspot_core::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: crate::persistence::s3fn8(shared_flash, S3_8_MIB_FLASH_LAYOUT.journal),
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    static NODE: StaticCell<Node> = StaticCell::new();
    let (node, persistence) =
        PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
    node.set_protocol_policy(personal_hopspot_core::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
    static PERSISTENCE: StaticCell<crate::persistence::S3Fn8Persistence> = StaticCell::new();
    let persistence = PERSISTENCE.init(persistence);

    spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam).expect("USB task fits"));
    spawner.spawn(
        ble_task(spawner, bluetooth, mac, ble_identity, ble_fleet).expect("Bluetooth task fits"),
    );
    lora.run(lora_seam).await
}
