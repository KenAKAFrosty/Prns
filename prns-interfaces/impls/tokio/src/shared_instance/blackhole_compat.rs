use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::string::String;
use std::sync::Arc;
use std::vec::Vec;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use prns_core::identity::IdentityHash;
use prns_core::interfaces::rns_management::RnsBlackholeTable;
use prns_core::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::units::InstantMillis;
use prns_runtime::runtime::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};
use rmpv::Value;

const VALUE_MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsBlackholeDecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    ExpectedIdentityHash,
    ExpectedEntryMap,
    InvalidSource,
    InvalidUntil,
    InvalidReason,
}

impl core::fmt::Display for RnsBlackholeDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::MessagePack => "invalid MessagePack",
            Self::TrailingData => "trailing data",
            Self::ExpectedMap => "expected an identity map",
            Self::ExpectedIdentityHash => "expected a binary identity hash",
            Self::ExpectedEntryMap => "expected a blackhole entry map",
            Self::InvalidSource => "invalid source identity",
            Self::InvalidUntil => "invalid until value",
            Self::InvalidReason => "invalid reason value",
        })
    }
}

impl std::error::Error for RnsBlackholeDecodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsBlackholeEncodeError;

impl core::fmt::Display for RnsBlackholeEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("could not encode MessagePack")
    }
}

impl std::error::Error for RnsBlackholeEncodeError {}

#[derive(Debug)]
pub enum RnsBlackholeFileError {
    Io(std::io::Error),
    Decode(RnsBlackholeDecodeError),
    Encode(RnsBlackholeEncodeError),
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
        decode_source(&bytes, local_source, now).map_err(Into::into)
    }

    pub fn store_local<Reason: AsRef<str>>(
        &self,
        local_source: IdentityHash,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Result<(), RnsBlackholeFileError> {
        let bytes = encode_local(local_source, entries).map_err(RnsBlackholeFileError::Encode)?;
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
        decode_source(&bytes, source, now).map_err(Into::into)
    }

    pub fn store_source<Reason: AsRef<str>>(
        &self,
        source: IdentityHash,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Result<(), RnsBlackholeFileError> {
        let bytes = encode_table(entries).map_err(RnsBlackholeFileError::Encode)?;
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

pub fn decode_source(
    bytes: &[u8],
    source: IdentityHash,
    now: InstantMillis,
) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeDecodeError> {
    decode(bytes, now, |_| Ok(source))
}

fn decode(
    bytes: &[u8],
    now: InstantMillis,
    mut entry_source: impl FnMut(&[(Value, Value)]) -> Result<IdentityHash, RnsBlackholeDecodeError>,
) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeDecodeError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value_with_max_depth(&mut cursor, VALUE_MAX_DEPTH)
        .map_err(|_| RnsBlackholeDecodeError::MessagePack)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(RnsBlackholeDecodeError::TrailingData);
    }
    let Value::Map(map) = value else {
        return Err(RnsBlackholeDecodeError::ExpectedMap);
    };

    let mut rows = Vec::with_capacity(map.len());
    let mut positions = HashMap::with_capacity(map.len());
    for (identity, entry) in &map {
        let Some(bytes) = identity_bytes(identity)? else {
            continue;
        };
        match positions.get(&bytes).copied() {
            Some(index) => rows[index] = (bytes, entry),
            None => {
                positions.insert(bytes, rows.len());
                rows.push((bytes, entry));
            }
        }
    }

    let mut decoded = Vec::with_capacity(rows.len());
    for (identity, entry) in rows {
        let Value::Map(fields) = entry else {
            return Err(RnsBlackholeDecodeError::ExpectedEntryMap);
        };
        let Some(expiry) = decode_persisted_expiry(field(fields, "until"), now)? else {
            continue;
        };
        let source = entry_source(fields)?;
        let reason = decode_reason(field(fields, "reason"))?;
        decoded.push(BlackholedIdentity {
            identity: IdentityHash::new(identity),
            source,
            expiry,
            reason,
        });
    }
    Ok(decoded)
}

pub fn encode_local<Reason: AsRef<str>>(
    local_source: IdentityHash,
    entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
) -> Result<Vec<u8>, RnsBlackholeEncodeError> {
    RnsBlackholeTable::from_entries(
        entries
            .into_iter()
            .filter(|entry| entry.source == local_source),
    )
    .encode_message_pack()
    .map_err(|_| RnsBlackholeEncodeError)
}

pub fn decode_table(
    bytes: &[u8],
    now: InstantMillis,
) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeDecodeError> {
    decode(bytes, now, |fields| {
        decode_source_field(field(fields, "source"))
    })
}

pub fn encode_table<Reason: AsRef<str>>(
    entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
) -> Result<Vec<u8>, RnsBlackholeEncodeError> {
    RnsBlackholeTable::from_entries(entries)
        .encode_message_pack()
        .map_err(|_| RnsBlackholeEncodeError)
}

fn identity_bytes(value: &Value) -> Result<Option<[u8; 16]>, RnsBlackholeDecodeError> {
    let Value::Binary(bytes) = value else {
        return Err(RnsBlackholeDecodeError::ExpectedIdentityHash);
    };
    Ok(bytes.as_slice().try_into().ok())
}

fn decode_source_field(value: Option<&Value>) -> Result<IdentityHash, RnsBlackholeDecodeError> {
    let Some(value) = value else {
        return Err(RnsBlackholeDecodeError::InvalidSource);
    };
    identity_bytes(value)?
        .map(IdentityHash::new)
        .ok_or(RnsBlackholeDecodeError::InvalidSource)
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

fn field<'a>(fields: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .rev()
        .find_map(|(key, value)| (key.as_str() == Some(name)).then_some(value))
}

fn decode_persisted_expiry(
    value: Option<&Value>,
    now: InstantMillis,
) -> Result<Option<BlackholeExpiry>, RnsBlackholeDecodeError> {
    let Some(value) = value else {
        return Ok(Some(BlackholeExpiry::Indefinite));
    };
    match value {
        Value::Nil => Ok(Some(BlackholeExpiry::Indefinite)),
        Value::Integer(integer) => {
            let Some(seconds) = integer.as_u64() else {
                if integer.as_i64().is_some() {
                    return Ok(None);
                }
                return Err(RnsBlackholeDecodeError::InvalidUntil);
            };
            let deadline = seconds.saturating_mul(1_000);
            Ok((now.0 < deadline).then_some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        Value::F32(_) | Value::F64(_) => {
            let Some(seconds) = value.as_f64() else {
                return Err(RnsBlackholeDecodeError::InvalidUntil);
            };
            if seconds == f64::INFINITY {
                return Ok(Some(BlackholeExpiry::At(InstantMillis(u64::MAX))));
            }
            let millis = seconds * 1_000.0;
            if !millis.is_finite() || millis <= now.0 as f64 {
                return Ok(None);
            }
            let deadline = if millis >= u64::MAX as f64 {
                u64::MAX
            } else {
                millis.floor() as u64
            };
            Ok(Some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        _ => Err(RnsBlackholeDecodeError::InvalidUntil),
    }
}

fn decode_reason(value: Option<&Value>) -> Result<Option<String>, RnsBlackholeDecodeError> {
    match value {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::String(reason)) => reason
            .as_str()
            .map(|reason| Some(reason.to_owned()))
            .ok_or(RnsBlackholeDecodeError::InvalidReason),
        Some(_) => Err(RnsBlackholeDecodeError::InvalidReason),
    }
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

    const RNS_138_FIXTURE: &[u8] = b"\x82\xc4\x10\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xcb\x41\xd9\x54\xfc\x40\x08\x00\x00\xa6reason\xa8operator\xc4\x10\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xc0\xa6reason\xc0";

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
    fn decodes_the_rns_138_umsgpack_file_and_applies_reload_expiry() {
        let decoded = decode_source(RNS_138_FIXTURE, source(), InstantMillis(1_700_000_000_124));
        assert_eq!(
            decoded,
            Ok(fixture_entries()
                .into_iter()
                .map(|entry| BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.map(String::from),
                })
                .collect())
        );

        let at_equality =
            decode_source(RNS_138_FIXTURE, source(), InstantMillis(1_700_000_000_125));
        assert_eq!(
            at_equality,
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x22; 16]),
                source: source(),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            }])
        );
    }

    #[test]
    fn encodes_exactly_what_rns_138_umsgpack_emits() {
        assert!(
            encode_local(source(), fixture_entries()).is_ok_and(|bytes| bytes == RNS_138_FIXTURE)
        );
    }

    #[test]
    fn local_encoding_excludes_entries_owned_by_a_remote_source() {
        let mut entries = fixture_entries();
        entries.push(BlackholedIdentity {
            identity: IdentityHash::new([0x33; 16]),
            source: IdentityHash::new([0xbb; 16]),
            expiry: BlackholeExpiry::Indefinite,
            reason: None,
        });
        assert!(encode_local(source(), entries).is_ok_and(|bytes| bytes == RNS_138_FIXTURE));
    }

    #[test]
    fn published_table_decoding_preserves_each_entry_source() {
        let decoded = decode_table(RNS_138_FIXTURE, InstantMillis(0));
        assert!(decoded.is_ok_and(|entries| {
            entries.len() == 2 && entries.iter().all(|entry| entry.source == source())
        }));

        let missing_source = Value::Map(vec![(
            Value::Binary(vec![0x44; 16]),
            Value::Map(vec![(Value::from("until"), Value::Nil)]),
        )]);
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &missing_source).unwrap();
        assert_eq!(
            decode_table(&bytes, InstantMillis(0)),
            Err(RnsBlackholeDecodeError::InvalidSource)
        );
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
        assert!(fs::read(file.local_path()).is_ok_and(|bytes| bytes == RNS_138_FIXTURE));
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

    #[test]
    fn malformed_shapes_fail_as_typed_decode_errors() {
        assert_eq!(
            decode_source(&[0x90], source(), InstantMillis(0)),
            Err(RnsBlackholeDecodeError::ExpectedMap)
        );
        assert_eq!(
            decode_source(&[0x80, 0x00], source(), InstantMillis(0)),
            Err(RnsBlackholeDecodeError::TrailingData)
        );
    }

    #[test]
    fn persisted_numeric_forms_follow_rns_reload_semantics() {
        let integer = source_map(Value::from(2));
        assert_eq!(
            decode_source(&integer, source(), InstantMillis(1_999)),
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x44; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(2_000)),
                reason: None,
            }])
        );
        assert!(decode_source(&integer, source(), InstantMillis(2_000))
            .is_ok_and(|entries| entries.is_empty()));

        let fractional = source_map(Value::F64(2.0005));
        assert_eq!(
            decode_source(&fractional, source(), InstantMillis(2_000)),
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x44; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(2_000)),
                reason: None,
            }])
        );

        for expired in [Value::from(-1), Value::from(0), Value::F64(f64::NAN)] {
            assert!(
                decode_source(&source_map(expired), source(), InstantMillis(0))
                    .is_ok_and(|entries| entries.is_empty())
            );
        }
    }

    #[test]
    fn source_identity_comes_from_the_source_file_and_short_hashes_are_skipped() {
        let replacement_source = IdentityHash::new([0xcc; 16]);
        let decoded = decode_source(RNS_138_FIXTURE, replacement_source, InstantMillis(0));
        assert!(decoded.is_ok_and(|entries| entries
            .iter()
            .all(|entry| entry.source == replacement_source)));

        let value = Value::Map(vec![
            (
                Value::Binary(vec![0x55; 15]),
                Value::Map(vec![(Value::from("until"), Value::Nil)]),
            ),
            (
                Value::Binary(vec![0x44; 16]),
                Value::Map(vec![(Value::from("until"), Value::Nil)]),
            ),
        ]);
        assert!(
            decode_source(&encode_value(value), source(), InstantMillis(0))
                .is_ok_and(|entries| entries.len() == 1)
        );
    }

    fn source_map(until: Value) -> Vec<u8> {
        encode_value(Value::Map(vec![(
            Value::Binary(vec![0x44; 16]),
            Value::Map(vec![(Value::from("until"), until)]),
        )]))
    }

    fn encode_value(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        assert!(rmpv::encode::write_value(&mut bytes, &value).is_ok());
        bytes
    }
}
