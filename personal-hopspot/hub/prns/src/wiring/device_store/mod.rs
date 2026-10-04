use super::controller_installation::InstallationLock;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::num::NonZeroU32;
use hopspot_hub_core::*;
use pipecircuit::StateMachine;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[cfg(test)]
mod behavior;
mod codec;
#[cfg(test)]
mod tests;

pub use codec::DeviceArchiveError;

const DEVICES_FILE: &str = "devices.hopspot";

pub struct DeviceStore {
    directory: PathBuf,
    _installation: Arc<InstallationLock>,
}

#[derive(Debug)]
pub enum DeviceStoreError {
    Io(std::io::Error),
    PublishedDurabilityUnconfirmed(std::io::Error),
    Archive(DeviceArchiveError),
    RegistryCreation(DeviceRegistryCreationError),
    Restore(Box<RestoreRememberedDeviceError>),
    RestoreRefused(Box<RestoreRememberedDeviceOutcome>),
    ArchiveTooLarge { maximum_bytes: u64 },
}

impl From<std::io::Error> for DeviceStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub enum LoadDevicesOutcome {
    Missing,
    Loaded { registry: DeviceRegistry },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum SaveDevicesOutcome {
    Saved,
    InsufficientCapacity { required: usize },
}

impl DeviceStore {
    pub(super) fn new(directory: PathBuf, installation: Arc<InstallationLock>) -> Self {
        Self {
            directory,
            _installation: installation,
        }
    }

    pub fn load(
        &self,
        maximum_devices: NonZeroU32,
    ) -> Result<LoadDevicesOutcome, DeviceStoreError> {
        let file = match File::open(self.directory.join(DEVICES_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadDevicesOutcome::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        let maximum_bytes = u64::from(maximum_devices.get())
            .saturating_mul(codec::MAX_RECORD_BYTES)
            .saturating_add(codec::ARCHIVE_OVERHEAD);
        let mut bytes = alloc::vec::Vec::new();
        file.take(maximum_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum_bytes {
            return Err(DeviceStoreError::ArchiveTooLarge { maximum_bytes });
        }
        let devices =
            codec::decode(&bytes, maximum_devices.get()).map_err(DeviceStoreError::Archive)?;
        DeviceRegistry::try_new(maximum_devices)
            .map_err(DeviceStoreError::RegistryCreation)
            .and_then(|mut registry| {
                devices
                    .into_iter()
                    .try_for_each(|remembered| {
                        settle_restoration(registry.step(RestoreRememberedDevice { remembered }))
                    })
                    .map(|()| LoadDevicesOutcome::Loaded { registry })
            })
    }

    pub fn save<const CAPACITY: usize>(
        &mut self,
        registry: &mut DeviceRegistry,
    ) -> Result<SaveDevicesOutcome, DeviceStoreError> {
        let devices = match registry.step(ReadRememberedDevices::<CAPACITY>) {
            ReadRememberedDevicesOutcome::Read { devices } => devices,
            ReadRememberedDevicesOutcome::InsufficientCapacity { required } => {
                return Ok(SaveDevicesOutcome::InsufficientCapacity { required });
            }
        };
        self.save_records(&devices)
            .map(|()| SaveDevicesOutcome::Saved)
    }

    fn save_records(&mut self, devices: &[RememberedDevice]) -> Result<(), DeviceStoreError> {
        let bytes = codec::encode(devices);
        let mut staged = NamedTempFile::new_in(&self.directory)?;
        stage_snapshot(staged.as_file_mut(), &bytes, File::sync_all)
            .map_err(DeviceStoreError::Io)
            .and_then(|()| self.publish(staged, confirm_directory))
    }

    fn publish(
        &self,
        staged: NamedTempFile,
        confirm: fn(&Path) -> std::io::Result<()>,
    ) -> Result<(), DeviceStoreError> {
        staged
            .persist(self.directory.join(DEVICES_FILE))
            .map_err(|error| DeviceStoreError::Io(error.error))?;
        confirm(&self.directory).map_err(DeviceStoreError::PublishedDurabilityUnconfirmed)
    }
}

fn confirm_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let absolute = std::fs::canonicalize(directory)?;
        absolute
            .ancestors()
            .try_for_each(|ancestor| File::open(ancestor).and_then(|file| file.sync_all()))
    }
    #[cfg(not(unix))]
    {
        let _directory = directory;
        Ok(())
    }
}

fn settle_restoration(
    outcome: Result<RestoreRememberedDeviceOutcome, RestoreRememberedDeviceError>,
) -> Result<(), DeviceStoreError> {
    outcome
        .map_err(Box::new)
        .map_err(DeviceStoreError::Restore)
        .and_then(|outcome| match outcome {
            RestoreRememberedDeviceOutcome::Restored { .. } => Ok(()),
            refused @ (RestoreRememberedDeviceOutcome::AtCapacity { .. }
            | RestoreRememberedDeviceOutcome::TargetAlreadyPaired { .. }) => {
                Err(DeviceStoreError::RestoreRefused(Box::new(refused)))
            }
        })
}

fn stage_snapshot(
    file: &mut File,
    bytes: &[u8],
    sync: fn(&File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    file.write_all(bytes)?;
    sync(file)
}

mod enrollment;
pub use enrollment::{PersistEnrollmentError, PersistEnrollmentOutcome};
