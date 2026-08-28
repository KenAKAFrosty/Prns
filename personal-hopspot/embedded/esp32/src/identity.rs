use core::convert::Infallible;
#[cfg(target_arch = "xtensa")]
use core::{cell::UnsafeCell, ffi::c_void, mem::MaybeUninit};

use esp_hal::rng::Rng;
#[cfg(target_arch = "xtensa")]
use personal_hopspot_core::HopspotS3FlashLayout;
#[cfg(target_arch = "xtensa")]
use personal_hopspot_core::UiNotice;
use personal_hopspot_core::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
    HopspotNodeIdentity, IdentityBootstrap, IdentityPersistence,
};
#[cfg(target_arch = "riscv32")]
use personal_hopspot_core::{
    ESP32_4_MIB_FLASH_CAPACITY, ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
};
use personal_rns::identity::vault::{FlashVault, FlashVaultError};
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::remote_control::{
    RemoteControlNodeIdentityBootstrap, RemoteControlNodeIdentityBootstrapError,
    REMOTE_CONTROL_IDENTITY_VAULT_SLOTS,
};
#[cfg(target_arch = "xtensa")]
use portable_atomic::{AtomicU8, Ordering};

use crate::flash::{EspRomFlash, EspRomFlashError};

const FLASH_SECTOR_LEN: u32 = 0x1000;
const BLE_IDENTITY_FLASH_OFFSET: u32 = 0xC000;
const HOPSPOT_CONFIG_FLASH_OFFSET: u32 = 0xD000;
const NODE_IDENTITY_FLASH_OFFSET: u32 = 0xE000;
const IDENTITY_FLASH_END: usize = 0xF000;
const VAULT_SLOTS: usize = 1;

const _: () = assert!(BLE_IDENTITY_FLASH_OFFSET + FLASH_SECTOR_LEN == HOPSPOT_CONFIG_FLASH_OFFSET);
const _: () = assert!(HOPSPOT_CONFIG_FLASH_OFFSET + FLASH_SECTOR_LEN == NODE_IDENTITY_FLASH_OFFSET);
const _: () = assert!(NODE_IDENTITY_FLASH_OFFSET + FLASH_SECTOR_LEN == IDENTITY_FLASH_END as u32);

pub(crate) type Error = FlashIdentityError<EspRomFlashError>;
pub(crate) type RemoteControlIdentityBootstrapError =
    RemoteControlNodeIdentityBootstrapError<FlashVaultError<EspRomFlashError>, Infallible>;

pub(crate) struct RemoteControlIdentityFlash {
    flash_capacity: usize,
    offset: u32,
}

impl RemoteControlIdentityFlash {
    const fn new(flash_capacity: usize, offset: u32) -> Self {
        Self {
            flash_capacity,
            offset,
        }
    }

    pub(crate) fn load_or_generate(
        self,
    ) -> Result<RemoteControlNodeIdentityBootstrap, RemoteControlIdentityBootstrapError> {
        let mut vault = FlashVault::<_, REMOTE_CONTROL_IDENTITY_VAULT_SLOTS>::new(
            EspRomFlash::new(self.flash_capacity),
            self.offset,
        );
        RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |bytes| {
            hardware_entropy(bytes);
            Ok(())
        })
    }
}

#[cfg(target_arch = "xtensa")]
impl From<HopspotS3FlashLayout> for RemoteControlIdentityFlash {
    fn from(layout: HopspotS3FlashLayout) -> Self {
        Self::new(
            layout.flash_capacity,
            layout.remote_control_identity_flash_offset,
        )
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) const C6_REMOTE_CONTROL_IDENTITY_FLASH: RemoteControlIdentityFlash =
    RemoteControlIdentityFlash::new(
        ESP32_4_MIB_FLASH_CAPACITY,
        ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    );

#[cfg(target_arch = "xtensa")]
const IDENTITY_TASK_IDLE: u8 = 0;
#[cfg(target_arch = "xtensa")]
const IDENTITY_TASK_RUNNING: u8 = 1;
#[cfg(target_arch = "xtensa")]
const IDENTITY_TASK_READY: u8 = 2;
#[cfg(target_arch = "xtensa")]
const IDENTITY_TASK_STACK_BYTES: usize = 40 * 1024;

#[cfg(target_arch = "xtensa")]
type IdentityBootstraps = (
    IdentityBootstrap<HopspotNodeIdentity, Error>,
    Result<RemoteControlNodeIdentityBootstrap, RemoteControlIdentityBootstrapError>,
    IdentityBootstrap<BleIdentity, Error>,
    personal_hopspot_core::HopspotDestinationHashes,
);

#[cfg(target_arch = "xtensa")]
struct IdentityTaskContext {
    state: AtomicU8,
    remote_control_identity_flash: UnsafeCell<MaybeUninit<RemoteControlIdentityFlash>>,
    output: UnsafeCell<MaybeUninit<IdentityBootstraps>>,
}

#[cfg(target_arch = "xtensa")]
impl IdentityTaskContext {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(IDENTITY_TASK_IDLE),
            remote_control_identity_flash: UnsafeCell::new(MaybeUninit::uninit()),
            output: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "the one-shot transition grants the caller exclusive initialization access before the worker starts"
    )]
    fn start(&self, remote_control_identity_flash: RemoteControlIdentityFlash) {
        self.state
            .compare_exchange(
                IDENTITY_TASK_IDLE,
                IDENTITY_TASK_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("S3 identities may only be bootstrapped once");
        unsafe {
            (*self.remote_control_identity_flash.get()).write(remote_control_identity_flash);
        }
    }

    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "the worker runs only after start initializes the one-shot input"
    )]
    fn take_remote_control_identity_flash(&self) -> RemoteControlIdentityFlash {
        unsafe { (*self.remote_control_identity_flash.get()).assume_init_read() }
    }
}

#[cfg(target_arch = "xtensa")]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "the one-shot state transition separates input initialization, worker output, and caller consumption"
)]
unsafe impl Sync for IdentityTaskContext {}

#[cfg(target_arch = "xtensa")]
static IDENTITY_TASK: IdentityTaskContext = IdentityTaskContext::new();

#[cfg(target_arch = "xtensa")]
extern "C" fn identity_task(_param: *mut c_void) {
    let remote_control_identity_flash = IDENTITY_TASK.take_remote_control_identity_flash();
    let node = bootstrap_node_identity();
    let remote_control = remote_control_identity_flash.load_or_generate();
    let destination_hashes =
        personal_hopspot_core::hopspot_destination_hashes(node.identity().secret())
            .expect("the built-in hopspot destination names are valid");
    let output = (
        node,
        remote_control,
        bootstrap_ble_identity(),
        destination_hashes,
    );

    // SAFETY: IDLE -> RUNNING is performed exactly once before this task is created, so no other
    // writer can access the output slot. The slot has static storage and outlives the task.
    unsafe { (*IDENTITY_TASK.output.get()).write(output) };
    IDENTITY_TASK
        .state
        .store(IDENTITY_TASK_READY, Ordering::Release);
}

/// Restores S3 identities and derives the local destination hashes away from the constrained
/// main stack.
///
/// Ed25519 public-key reconstruction has a large stack high-water mark. Running it on a temporary
/// radio-RTOS task keeps the main stack guard effective and frees the temporary stack when the
/// worker exits. The persisted records and bootstrap behavior are otherwise identical.
#[cfg(target_arch = "xtensa")]
pub(crate) async fn bootstrap_s3_identities(
    remote_control_identity_flash: RemoteControlIdentityFlash,
) -> IdentityBootstraps {
    IDENTITY_TASK.start(remote_control_identity_flash);

    // SAFETY: `IDENTITY_TASK` has static storage and is Send. The worker only uses the global
    // directly, but passing its address documents and satisfies the RTOS task parameter lifetime.
    unsafe {
        esp_radio_rtos_driver::task_create(
            "identity-bootstrap",
            identity_task,
            (&raw const IDENTITY_TASK).cast_mut().cast::<c_void>(),
            1,
            Some(0),
            IDENTITY_TASK_STACK_BYTES,
        )
    };

    while IDENTITY_TASK.state.load(Ordering::Acquire) != IDENTITY_TASK_READY {
        embassy_time::Timer::after_millis(1).await;
    }

    // SAFETY: READY is published only after the worker fully initializes the slot. This function
    // is one-shot, so the initialized value is moved out exactly once.
    unsafe { (*IDENTITY_TASK.output.get()).assume_init_read() }
}

pub fn log_persistence(identity: &str, persistence: &IdentityPersistence<Error>) {
    match persistence {
        IdentityPersistence::Loaded => {}
        IdentityPersistence::Created => log::info!("{identity} identity created"),
        IdentityPersistence::Recovered(error) => {
            log::warn!("{identity} identity recovered after corruption: {error}")
        }
        IdentityPersistence::Ephemeral(error) => {
            log::error!("{identity} identity is ephemeral: {error}")
        }
    }
}

#[cfg(target_arch = "xtensa")]
pub fn startup_notice(
    node: &IdentityPersistence<Error>,
    bluetooth: &IdentityPersistence<Error>,
) -> Option<UiNotice> {
    if node.is_ephemeral() || bluetooth.is_ephemeral() {
        Some(UiNotice::IdentityUnstable)
    } else if node.is_recovered() || bluetooth.is_recovered() {
        Some(UiNotice::IdentityReset)
    } else {
        None
    }
}

pub fn bootstrap_node_identity() -> IdentityBootstrap<HopspotNodeIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(
        EspRomFlash::new(IDENTITY_FLASH_END),
        NODE_IDENTITY_FLASH_OFFSET,
    );
    bootstrap_flash_node_identity(&mut vault, &mut hardware_entropy)
}

pub fn bootstrap_ble_identity() -> IdentityBootstrap<BleIdentity, Error> {
    let mut vault = FlashVault::<_, VAULT_SLOTS>::new(
        EspRomFlash::new(IDENTITY_FLASH_END),
        BLE_IDENTITY_FLASH_OFFSET,
    );
    bootstrap_flash_ble_identity(&mut vault, &mut hardware_entropy)
}

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}
