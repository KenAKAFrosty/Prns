use personal_rns::prelude::{
    RemoteControlFileIdentityBootstrapError, RemoteControlIdentityDirectory,
    RemoteControlNodeIdentityBootstrap,
};
use personal_rns::runtime::NodePersistence;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub enum ControllerInstallationError {
    StateDirectory(std::io::Error),
    LockFile(std::io::Error),
    Lock(TryLockError),
    Identity(RemoteControlFileIdentityBootstrapError),
    Persistence(std::io::Error),
}

pub struct ControllerInstallation {
    pub(crate) state_lock: InstallationLock,
    pub(crate) identity: RemoteControlNodeIdentityBootstrap,
    pub(crate) persistence: NodePersistence,
}

pub(crate) struct InstallationLock(File);

impl Drop for InstallationLock {
    fn drop(&mut self) {
        let _unlock = self.0.unlock();
    }
}

impl ControllerInstallation {
    pub fn open(directory: &Path) -> Result<Self, ControllerInstallationError> {
        prepare_directory(directory).map_err(ControllerInstallationError::StateDirectory)?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let state_lock = options
            .open(directory.join("hub.lock"))
            .map_err(ControllerInstallationError::LockFile)?;
        state_lock
            .try_lock()
            .map_err(ControllerInstallationError::Lock)?;
        let state_lock = InstallationLock(state_lock);
        let identity = RemoteControlIdentityDirectory::new(directory.join("remote_control"))
            .load_or_generate()
            .map_err(ControllerInstallationError::Identity)?;
        let persistence = NodePersistence::custom_dir(directory.join("retained"))
            .map_err(ControllerInstallationError::Persistence)?;
        Ok(Self {
            state_lock,
            identity,
            persistence,
        })
    }
}

fn prepare_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        Ok(())
    }
}
