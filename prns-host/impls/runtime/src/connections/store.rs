use super::model::{normalize, random_id, storage};
use super::{Error, Guard, Limits, Options, Profile, SchemaError, Snapshot};
use prns_storage::database;
use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTableMetadata, TableDefinition,
    WriteTransaction,
};
use std::{path::Path, sync::Arc};

const TABLE: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_host_connections");
const MAGIC: &[u8; 4] = b"PHCP";
const VERSION: u32 = 1;
const HEADER: usize = 40;

fn physical(error: database::Error<Error>) -> Error {
    match error {
        database::Error::InvalidOptions => Error::Invalid(super::InputError::InvalidLimits),
        database::Error::File(e) => Error::File(Arc::new(e)),
        database::Error::Database(e) => storage(e),
        database::Error::Domain(e) => e,
    }
}
fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, Error> {
    let profiles =
        serde_json::to_vec(&snapshot.profiles).map_err(|_| Error::Schema(SchemaError::Corrupt))?;
    if HEADER + profiles.len() > snapshot.limits.encoded_bytes as usize {
        return Err(Error::Invalid(super::InputError::EncodedCapacity));
    }
    let mut bytes = Vec::with_capacity(HEADER + profiles.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&snapshot.guard.incarnation);
    bytes.extend_from_slice(&snapshot.guard.revision.to_le_bytes());
    bytes.extend_from_slice(&snapshot.limits.profiles.to_le_bytes());
    bytes.extend_from_slice(&snapshot.limits.encoded_bytes.to_le_bytes());
    bytes.extend_from_slice(&profiles);
    Ok(bytes)
}
fn decode(bytes: &[u8], limits: Limits) -> Result<Snapshot, Error> {
    let corrupt = || Error::Schema(SchemaError::Corrupt);
    if bytes.len() < HEADER || bytes.len() > limits.encoded_bytes as usize || &bytes[..4] != MAGIC {
        return Err(corrupt());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| corrupt())?);
    if version != VERSION {
        return Err(Error::Schema(SchemaError::UnsupportedVersion(version)));
    }
    let incarnation = bytes[8..24].try_into().map_err(|_| corrupt())?;
    if incarnation == [0; 16] {
        return Err(corrupt());
    }
    let revision = u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| corrupt())?);
    let stored_limits = Limits {
        profiles: u32::from_le_bytes(bytes[32..36].try_into().map_err(|_| corrupt())?),
        encoded_bytes: u32::from_le_bytes(bytes[36..40].try_into().map_err(|_| corrupt())?),
    };
    if stored_limits != limits {
        return Err(Error::Schema(SchemaError::CapacityMismatch));
    }
    let profiles: Vec<Profile> = serde_json::from_slice(&bytes[HEADER..]).map_err(|_| corrupt())?;
    let normalized = normalize(profiles.clone(), limits).map_err(|_| corrupt())?;
    if normalized != profiles {
        return Err(corrupt());
    }
    Ok(Snapshot {
        guard: Guard {
            incarnation,
            revision,
        },
        limits,
        profiles,
    })
}
fn read_in(tx: &ReadTransaction, limits: Limits) -> Result<Snapshot, Error> {
    let table = tx.open_table(TABLE).map_err(|e| match e {
        redb::TableError::TableDoesNotExist(_) => Error::Schema(SchemaError::Missing),
        e => storage(e),
    })?;
    if table.len().map_err(storage)? != 1 {
        return Err(Error::Schema(SchemaError::Corrupt));
    }
    let value = table
        .get(0)
        .map_err(storage)?
        .ok_or(Error::Schema(SchemaError::Missing))?;
    decode(value.value(), limits)
}
fn write_in(tx: &WriteTransaction, snapshot: &Snapshot) -> Result<(), Error> {
    let bytes = encode(snapshot)?;
    tx.open_table(TABLE)
        .map_err(storage)?
        .insert(0, bytes.as_slice())
        .map_err(storage)?;
    Ok(())
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum Fault {
    BeforeCommit,
    AfterCommit,
}
#[cfg(test)]
fn injected() -> Error {
    Error::File(Arc::new(prns_storage::files::Error::Io(
        std::io::Error::other("injected commit confirmation failure"),
    )))
}

pub(super) struct Store {
    owner: database::Owner,
    limits: Limits,
    #[cfg(test)]
    fault: std::cell::Cell<Option<Fault>>,
}
impl Store {
    pub fn open(
        path: &Path,
        initial: Option<Vec<Profile>>,
        options: Options,
    ) -> Result<Self, Error> {
        let physical_options = database::Options {
            cache_bytes: options.cache_bytes,
        };
        let owner = if let Some(profiles) = initial {
            let snapshot = Snapshot {
                guard: Guard {
                    incarnation: random_id()?,
                    revision: 0,
                },
                limits: options.limits,
                profiles,
            };
            // Encoding/capacity errors precede even file creation.
            encode(&snapshot)?;
            database::Owner::create_new(path, physical_options, |tx| write_in(tx, &snapshot))
                .map_err(physical)?
        } else {
            database::Owner::open_existing(path, physical_options, |tx| {
                read_in(tx, options.limits).map(|_| ())
            })
            .map_err(physical)?
        };
        Ok(Self {
            owner,
            limits: options.limits,
            #[cfg(test)]
            fault: std::cell::Cell::new(None),
        })
    }
    #[cfg(test)]
    pub fn inject(&self, fault: Fault) {
        self.fault.set(Some(fault));
    }
    fn db(&self) -> &Database {
        self.owner.database()
    }
    pub fn read(&self) -> Result<Snapshot, Error> {
        read_in(&self.db().begin_read().map_err(storage)?, self.limits)
    }
    pub fn replace(&self, expected: Guard, profiles: Vec<Profile>) -> Result<Snapshot, Error> {
        let current = self.read()?;
        if current.guard != expected {
            return Err(Error::Conflict(current));
        }
        if current.profiles == profiles {
            return Ok(current);
        }
        let snapshot = Snapshot {
            guard: Guard {
                incarnation: expected.incarnation,
                revision: expected
                    .revision
                    .checked_add(1)
                    .ok_or(Error::Schema(SchemaError::RevisionExhausted))?,
            },
            limits: self.limits,
            profiles,
        };
        encode(&snapshot)?;
        let mut tx = self.db().begin_write().map_err(storage)?;
        tx.set_durability(redb::Durability::Immediate)
            .map_err(storage)?;
        write_in(&tx, &snapshot)?;
        #[cfg(test)]
        let fault = self.fault.take();
        #[cfg(test)]
        if matches!(fault, Some(Fault::BeforeCommit)) {
            return Err(injected());
        }
        tx.commit().map_err(storage)?;
        #[cfg(test)]
        if matches!(fault, Some(Fault::AfterCommit)) {
            return Err(injected());
        }
        Ok(snapshot)
    }
}

pub(super) fn checked(profiles: Vec<Profile>, limits: Limits) -> Result<Vec<Profile>, Error> {
    let profiles = normalize(profiles, limits)?;
    encode(&Snapshot {
        guard: Guard {
            incarnation: [1; 16],
            revision: 0,
        },
        limits,
        profiles: profiles.clone(),
    })?;
    Ok(profiles)
}

#[cfg(test)]
mod tests;
