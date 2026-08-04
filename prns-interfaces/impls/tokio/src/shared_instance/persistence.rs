use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::string::String;
use std::sync::Arc;
use std::vec::Vec;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use prns_core::identity::IdentityHash;
use prns_core::interfaces::rns_management::{
    RnsBlackholeDecodeError, RnsBlackholeTable, RnsManagementEncodeError,
};
use prns_core::routing::{
    BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::units::InstantMillis;
use prns_runtime::runtime::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};

#[derive(Debug)]
pub enum RnsBlackholeFileError {
    Io(std::io::Error),
    Decode(RnsBlackholeDecodeError),
    Encode(RnsManagementEncodeError),
}

impl core::fmt::Display for RnsBlackholeFileError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Decode(error) => write!(formatter, "invalid RNS blackhole file: {error}"),
            Self::Encode(error) => {
                write!(formatter, "could not encode RNS blackhole file: {error}")
            }
        }
    }
}

impl std::error::Error for RnsBlackholeFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Encode(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for RnsBlackholeFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<RnsBlackholeDecodeError> for RnsBlackholeFileError {
    fn from(error: RnsBlackholeDecodeError) -> Self {
        Self::Decode(error)
    }
}

#[derive(Debug, Clone)]
pub struct RnsBlackholeFiles {
    blackhole_dir: PathBuf,
}

impl RnsBlackholeFiles {
    pub fn new(blackhole_dir: impl Into<PathBuf>) -> Self {
        Self {
            blackhole_dir: blackhole_dir.into(),
        }
    }

    pub fn local_path(&self) -> PathBuf {
        self.blackhole_dir.join("local")
    }

    pub fn source_path(&self, source: IdentityHash) -> PathBuf {
        self.blackhole_dir.join(identity_hex(source))
    }

    pub fn load_local(
        &self,
        local_source: IdentityHash,
        now: InstantMillis,
    ) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeFileError> {
        let bytes = match fs::read(self.local_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        RnsBlackholeTable::decode_source_file(&bytes, local_source, now)
            .map(RnsBlackholeTable::into_entries)
            .map_err(Into::into)
    }

    pub fn store_local<Reason: AsRef<str>>(
        &self,
        local_source: IdentityHash,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Result<(), RnsBlackholeFileError> {
        let bytes = RnsBlackholeTable::from_source_entries(local_source, entries)
            .encode_message_pack()
            .map_err(RnsBlackholeFileError::Encode)?;
        self.store_path(self.local_path(), bytes)
    }

    pub fn load_source(
        &self,
        source: IdentityHash,
        now: InstantMillis,
    ) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeFileError> {
        let bytes = match fs::read(self.source_path(source)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        RnsBlackholeTable::decode_source_file(&bytes, source, now)
            .map(RnsBlackholeTable::into_entries)
            .map_err(Into::into)
    }

    pub fn store_source<Reason: AsRef<str>>(
        &self,
        source: IdentityHash,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Result<(), RnsBlackholeFileError> {
        let bytes = RnsBlackholeTable::from_entries(entries)
            .encode_message_pack()
            .map_err(RnsBlackholeFileError::Encode)?;
        self.store_path(self.source_path(source), bytes)
    }

    fn ensure_dir(&self) -> Result<(), RnsBlackholeFileError> {
        if !self.blackhole_dir.exists() {
            fs::create_dir_all(&self.blackhole_dir)?;
            #[cfg(unix)]
            let _ = fs::set_permissions(&self.blackhole_dir, fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }

    fn store_path(&self, final_path: PathBuf, bytes: Vec<u8>) -> Result<(), RnsBlackholeFileError> {
        self.ensure_dir()?;
        let staging_path = final_path.with_extension("tmp");
        let result = stage(&staging_path, &bytes).and_then(|()| {
            replace_file(&staging_path, &final_path).map_err(RnsBlackholeFileError::from)
        });
        if result.is_err() {
            let _ = fs::remove_file(staging_path);
        }
        result
    }
}

#[derive(Clone)]
pub struct RnsPersistedBlackholes<C> {
    inner: C,
    local_source: IdentityHash,
    files: RnsBlackholeFiles,
    mutation: Arc<tokio::sync::Mutex<()>>,
}

impl<C> RnsPersistedBlackholes<C> {
    pub fn new(inner: C, local_source: IdentityHash, files: RnsBlackholeFiles) -> Self {
        Self {
            inner,
            local_source,
            files,
            mutation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

impl<C> IdentityBlackholeSource for RnsPersistedBlackholes<C>
where
    C: IdentityBlackholeSource + Sync,
{
    type Reason = C::Reason;
    type Entries = C::Entries;

    fn blackholed_identities(
        &self,
    ) -> impl core::future::Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send
    {
        self.inner.blackholed_identities()
    }

    fn is_blackholed(
        &self,
        identity: IdentityHash,
    ) -> impl core::future::Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send {
        self.inner.is_blackholed(identity)
    }
}

impl<C> IdentityBlackholeControl for RnsPersistedBlackholes<C>
where
    C: IdentityBlackholeSource + IdentityBlackholeControl + Sync,
{
    async fn blackhole_identity<'a>(
        &'a self,
        entry: BlackholedIdentity<&'a str>,
    ) -> Result<BlackholeIdentityOutcome, IdentityBlackholeControlError> {
        let _mutation = self.mutation.lock().await;
        let outcome = self.inner.blackhole_identity(entry).await?;
        if outcome == BlackholeIdentityOutcome::Added {
            self.persist().await?;
        }
        Ok(outcome)
    }

    async fn unblackhole_identity(
        &self,
        identity: IdentityHash,
    ) -> Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError> {
        let _mutation = self.mutation.lock().await;
        let outcome = self.inner.unblackhole_identity(identity).await?;
        if outcome == UnblackholeIdentityOutcome::Removed {
            self.persist().await?;
        }
        Ok(outcome)
    }
}

impl<C> RnsPersistedBlackholes<C>
where
    C: IdentityBlackholeSource + Sync,
{
    async fn persist(&self) -> Result<(), IdentityBlackholeControlError> {
        let entries = self
            .inner
            .blackholed_identities()
            .await
            .map_err(source_control_error)?;
        self.files
            .store_local(self.local_source, entries)
            .map_err(|_| IdentityBlackholeControlError::DurabilityFailed)
    }
}

fn source_control_error(error: IdentityBlackholeSourceError) -> IdentityBlackholeControlError {
    match error {
        IdentityBlackholeSourceError::NodeStopped => IdentityBlackholeControlError::NodeStopped,
        IdentityBlackholeSourceError::Busy => IdentityBlackholeControlError::Busy,
    }
}

fn identity_hex(identity: IdentityHash) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(32);
    for byte in identity.as_bytes() {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    text
}

fn stage(path: &Path, bytes: &[u8]) -> Result<(), RnsBlackholeFileError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn replace_file(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    fs::rename(staging, final_path)
}

#[cfg(not(unix))]
fn replace_file(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    match fs::remove_file(final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(staging, final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::routing::BlackholeExpiry;

    const RNS_1_4_2_FIXTURE: &[u8] = b"\x82\xc4\x10\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xcb\x41\xd9\x54\xfc\x40\x08\x00\x00\xa6reason\xa8operator\xc4\x10\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xc0\xa6reason\xc0";

    fn source() -> IdentityHash {
        IdentityHash::new([0xaa; 16])
    }

    fn fixture_entries() -> Vec<BlackholedIdentity<&'static str>> {
        vec![
            BlackholedIdentity {
                identity: IdentityHash::new([0x11; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(1_700_000_000_125)),
                reason: Some("operator"),
            },
            BlackholedIdentity {
                identity: IdentityHash::new([0x22; 16]),
                source: source(),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            },
        ]
    }

    #[derive(Clone, Default)]
    struct MemoryBlackholes {
        entries: Arc<std::sync::Mutex<Vec<BlackholedIdentity<String>>>>,
    }

    impl IdentityBlackholeSource for MemoryBlackholes {
        type Reason = String;
        type Entries = Vec<BlackholedIdentity<String>>;

        fn blackholed_identities(
            &self,
        ) -> impl core::future::Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send
        {
            let entries = self.entries.lock().unwrap().clone();
            std::future::ready(Ok(entries))
        }

        fn is_blackholed(
            &self,
            identity: IdentityHash,
        ) -> impl core::future::Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send
        {
            let found = self
                .entries
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.identity == identity);
            std::future::ready(Ok(found))
        }
    }

    impl IdentityBlackholeControl for MemoryBlackholes {
        fn blackhole_identity<'a>(
            &'a self,
            entry: BlackholedIdentity<&'a str>,
        ) -> impl core::future::Future<
            Output = Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>,
        > + Send
               + 'a {
            let mut entries = self.entries.lock().unwrap();
            let outcome = if entries
                .iter()
                .any(|stored| stored.identity == entry.identity)
            {
                BlackholeIdentityOutcome::AlreadyPresent
            } else {
                entries.push(BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.map(String::from),
                });
                BlackholeIdentityOutcome::Added
            };
            drop(entries);
            std::future::ready(Ok(outcome))
        }

        fn unblackhole_identity(
            &self,
            identity: IdentityHash,
        ) -> impl core::future::Future<
            Output = Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
        > + Send {
            let mut entries = self.entries.lock().unwrap();
            let outcome = match entries.iter().position(|entry| entry.identity == identity) {
                Some(index) => {
                    entries.swap_remove(index);
                    UnblackholeIdentityOutcome::Removed
                }
                None => UnblackholeIdentityOutcome::NotFound,
            };
            drop(entries);
            std::future::ready(Ok(outcome))
        }
    }

    #[test]
    fn remote_source_files_use_the_direct_source_name_and_override_it_on_reload() {
        let dir = std::env::temp_dir().join(format!(
            "prns-rns-remote-blackhole-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let files = RnsBlackholeFiles::new(&dir);
        let direct_source = IdentityHash::new([0xbb; 16]);

        assert!(files.store_source(direct_source, fixture_entries()).is_ok());
        assert_eq!(
            files.source_path(direct_source),
            dir.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert!(files
            .load_source(direct_source, InstantMillis(0))
            .is_ok_and(|entries| entries.iter().all(|entry| entry.source == direct_source)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_store_replaces_through_local_tmp_and_missing_load_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "prns-rns-blackhole-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let file = RnsBlackholeFiles::new(&dir);
        assert!(file
            .load_local(source(), InstantMillis(0))
            .is_ok_and(|rows| rows.is_empty()));

        assert!(file.store_local(source(), fixture_entries()).is_ok());
        assert!(fs::read(file.local_path()).is_ok_and(|bytes| bytes == RNS_1_4_2_FIXTURE));
        assert!(!dir.join("local.tmp").exists());

        assert!(file
            .store_local(source(), Vec::<BlackholedIdentity<&str>>::new())
            .is_ok());
        assert!(fs::read(file.local_path()).is_ok_and(|bytes| bytes == [0x80]));
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn persisted_capability_commits_local_mutations_in_rns_format() {
        let dir = std::env::temp_dir().join(format!(
            "prns-rns-persisted-blackhole-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let file = RnsBlackholeFiles::new(&dir);
        let inner = MemoryBlackholes::default();
        let blackholes = RnsPersistedBlackholes::new(inner.clone(), source(), file.clone());
        let local_identity = IdentityHash::new([0x31; 16]);
        let remote_identity = IdentityHash::new([0x32; 16]);

        assert_eq!(
            blackholes
                .blackhole_identity(BlackholedIdentity {
                    identity: local_identity,
                    source: source(),
                    expiry: BlackholeExpiry::Indefinite,
                    reason: Some("operator"),
                })
                .await,
            Ok(BlackholeIdentityOutcome::Added)
        );
        assert!(file
            .load_local(source(), InstantMillis(0))
            .is_ok_and(|entries| entries.len() == 1 && entries[0].identity == local_identity));

        assert_eq!(
            blackholes
                .blackhole_identity(BlackholedIdentity {
                    identity: remote_identity,
                    source: IdentityHash::new([0xbb; 16]),
                    expiry: BlackholeExpiry::Indefinite,
                    reason: None,
                })
                .await,
            Ok(BlackholeIdentityOutcome::Added)
        );
        assert!(file
            .load_local(source(), InstantMillis(0))
            .is_ok_and(|entries| entries.len() == 1 && entries[0].identity == local_identity));

        assert_eq!(
            blackholes.unblackhole_identity(local_identity).await,
            Ok(UnblackholeIdentityOutcome::Removed)
        );
        assert!(file
            .load_local(source(), InstantMillis(0))
            .is_ok_and(|entries| entries.is_empty()));
        assert_eq!(
            inner.blackholed_identities().await,
            Ok(vec![BlackholedIdentity {
                identity: remote_identity,
                source: IdentityHash::new([0xbb; 16]),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            }])
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn durability_failure_is_typed_after_the_live_mutation() {
        let root = std::env::temp_dir().join(format!(
            "prns-rns-blackhole-failure-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::write(&root, b"not a directory").unwrap();
        let inner = MemoryBlackholes::default();
        let blackholes = RnsPersistedBlackholes::new(
            inner.clone(),
            source(),
            RnsBlackholeFiles::new(root.join("blackhole")),
        );
        let identity = IdentityHash::new([0x31; 16]);

        assert_eq!(
            blackholes
                .blackhole_identity(BlackholedIdentity {
                    identity,
                    source: source(),
                    expiry: BlackholeExpiry::Indefinite,
                    reason: None,
                })
                .await,
            Err(IdentityBlackholeControlError::DurabilityFailed)
        );
        assert_eq!(inner.is_blackholed(identity).await, Ok(true));
        let _ = fs::remove_file(root);
    }
}
