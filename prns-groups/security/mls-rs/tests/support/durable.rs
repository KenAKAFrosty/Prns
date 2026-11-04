use prns_groups_store::{
    AtomicBackend, CommitOutcome, CommitReceipt, Mutation, OperationId, RecoveryOutcome, Revision,
};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

const MAGIC: &[u8; 8] = b"MLSREC01";
const MAX_FIXTURE_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub enum Cut {
    BeforeSnapshot,
    DuringSnapshot,
    AfterSnapshot,
    AfterPublish,
}

#[derive(Clone, Default)]
pub struct Faults(Arc<Mutex<Option<Cut>>>);

impl Faults {
    pub fn arm(&self, cut: Cut) {
        *self.0.lock().unwrap() = Some(cut);
    }

    fn take(&self) -> Option<Cut> {
        self.0.lock().unwrap().take()
    }
}

pub struct Directory(PathBuf);

impl Directory {
    pub fn new() -> io::Result<Self> {
        let process = std::process::id();
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("prns-mls-recovery-{process}-{id}"));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Host-only snapshot fixture. Faults leave real files behind; reopening creates
/// fresh state from the published image. This does not model NOR erase ordering,
/// physical key erasure, hostile rollback, or host power-loss guarantees.
pub struct FileBackend {
    directory: PathBuf,
    faults: Faults,
}

#[derive(Default)]
struct Image {
    revision: u64,
    values: BTreeMap<Vec<u8>, Vec<u8>>,
    receipts: BTreeMap<[u8; 32], u64>,
}

impl FileBackend {
    pub fn open(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            faults: Faults::default(),
        })
    }

    pub fn faults(&self) -> Faults {
        self.faults.clone()
    }

    pub fn published_bytes(&self) -> io::Result<Vec<u8>> {
        match fs::read(self.directory.join("committed")) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn committed(&self) -> io::Result<Option<File>> {
        match File::open(self.directory.join("committed")) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn image(&self) -> io::Result<Image> {
        let Some(mut file) = self.committed()? else {
            return Ok(Image::default());
        };
        let (revision, entries, receipts) = read_header(&mut file)?;
        let mut image = Image {
            revision,
            ..Image::default()
        };
        for _ in 0..entries {
            let key_len = read_u32(&mut file)? as usize;
            let value_len = checked_length(read_u64(&mut file)?)?;
            let key = read_bytes(&mut file, key_len)?;
            let value = read_bytes(&mut file, value_len)?;
            if image.values.insert(key, value).is_some() {
                return Err(invalid("duplicate key"));
            }
        }
        for _ in 0..receipts {
            let mut operation = [0; 32];
            file.read_exact(&mut operation)?;
            let revision = read_u64(&mut file)?;
            if image.receipts.insert(operation, revision).is_some() {
                return Err(invalid("duplicate operation"));
            }
        }
        if file.stream_position()? != file.metadata()?.len() {
            return Err(invalid("trailing snapshot bytes"));
        }
        Ok(image)
    }
}

impl AtomicBackend for FileBackend {
    type Error = io::Error;

    fn head(&mut self) -> io::Result<Revision> {
        let Some(mut file) = self.committed()? else {
            return Ok(Revision(0));
        };
        Ok(Revision(read_header(&mut file)?.0))
    }

    fn read(&mut self, wanted: &[u8], max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        let Some(mut file) = self.committed()? else {
            return Ok(None);
        };
        let (_, entries, _) = read_header(&mut file)?;
        for _ in 0..entries {
            let key_len = read_u32(&mut file)? as usize;
            let value_len = checked_length(read_u64(&mut file)?)?;
            let key = read_bytes(&mut file, key_len)?;
            if key == wanted {
                if value_len > max_bytes {
                    return Err(invalid("value exceeds caller read limit"));
                }
                return Ok(Some(read_bytes(&mut file, value_len)?));
            }
            file.seek(SeekFrom::Current(value_len as i64))?;
        }
        Ok(None)
    }

    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> io::Result<CommitOutcome> {
        let mut image = self.image()?;
        if let Some(revision) = image.receipts.get(&operation.0) {
            return Ok(CommitOutcome::Committed(CommitReceipt {
                operation_id: operation,
                revision: Revision(*revision),
            }));
        }
        if image.revision != expected.0 {
            return Ok(CommitOutcome::NotCommitted {
                revision: Revision(image.revision),
            });
        }
        let cut = self.faults.take();
        if matches!(cut, Some(Cut::BeforeSnapshot)) {
            return Ok(CommitOutcome::Uncertain);
        }
        for mutation in changes {
            match mutation {
                Mutation::Put { key, value } => {
                    image.values.insert(key.clone(), value.clone());
                }
                Mutation::Delete { key } => {
                    image.values.remove(key);
                }
            }
        }
        image.revision = image
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("revision overflow"))?;
        image.receipts.insert(operation.0, image.revision);
        let bytes = image.encode()?;
        let pending = self.directory.join("pending");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&pending)?;
        if matches!(cut, Some(Cut::DuringSnapshot)) {
            file.write_all(&bytes[..bytes.len() / 2])?;
            file.sync_all()?;
            return Ok(CommitOutcome::Uncertain);
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if matches!(cut, Some(Cut::AfterSnapshot)) {
            return Ok(CommitOutcome::Uncertain);
        }
        fs::rename(pending, self.directory.join("committed"))?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        if matches!(cut, Some(Cut::AfterPublish)) {
            return Ok(CommitOutcome::Uncertain);
        }
        Ok(CommitOutcome::Committed(CommitReceipt {
            operation_id: operation,
            revision: Revision(image.revision),
        }))
    }

    fn recover(&mut self, operation: OperationId) -> io::Result<RecoveryOutcome> {
        let image = self.image()?;
        Ok(match image.receipts.get(&operation.0) {
            Some(revision) => RecoveryOutcome::Committed(CommitReceipt {
                operation_id: operation,
                revision: Revision(*revision),
            }),
            None => RecoveryOutcome::NotCommitted {
                revision: Revision(image.revision),
            },
        })
    }
}

impl Image {
    fn encode(&self) -> io::Result<Vec<u8>> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&self.revision.to_be_bytes());
        bytes.extend_from_slice(&u32_length(self.values.len())?.to_be_bytes());
        bytes.extend_from_slice(&u32_length(self.receipts.len())?.to_be_bytes());
        for (key, value) in &self.values {
            bytes.extend_from_slice(&u32_length(key.len())?.to_be_bytes());
            bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
            bytes.extend_from_slice(key);
            bytes.extend_from_slice(value);
        }
        for (operation, revision) in &self.receipts {
            bytes.extend_from_slice(operation);
            bytes.extend_from_slice(&revision.to_be_bytes());
        }
        checked_length(bytes.len() as u64)?;
        Ok(bytes)
    }
}

fn read_header(file: &mut File) -> io::Result<(u64, u32, u32)> {
    checked_length(file.metadata()?.len())?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if magic != *MAGIC {
        return Err(invalid("unknown snapshot format"));
    }
    Ok((read_u64(file)?, read_u32(file)?, read_u32(file)?))
}

fn read_u32(file: &mut File) -> io::Result<u32> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(file: &mut File) -> io::Result<u64> {
    let mut bytes = [0; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

fn read_bytes(file: &mut File, length: usize) -> io::Result<Vec<u8>> {
    checked_length(length as u64)?;
    let remaining = file
        .metadata()?
        .len()
        .saturating_sub(file.stream_position()?);
    if length as u64 > remaining {
        return Err(invalid("truncated snapshot"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| invalid("snapshot read allocation failed"))?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn checked_length(length: u64) -> io::Result<usize> {
    if length > MAX_FIXTURE_BYTES {
        return Err(invalid("snapshot fixture limit"));
    }
    usize::try_from(length).map_err(|_| invalid("length overflow"))
}

fn u32_length(length: usize) -> io::Result<u32> {
    u32::try_from(length).map_err(|_| invalid("entry count overflow"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
