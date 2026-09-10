use redb::{Database, ReadableDatabase, ReadableTable};
use serde::{Deserialize, Deserializer, Serialize};

use crate::contract::{Contact, ContactListOutcome, ContactLookupOutcome, ContactMutationOutcome};
use crate::development_store::{
    classify_redb_error, DevelopmentStoreFailure, StoreReply, CONTACTS,
};

#[derive(Debug)]
pub(crate) enum DirectoryRequest {
    SaveObserved {
        destination: [u8; 16],
        identity: [u8; 16],
    },
    CreateManual {
        destination: [u8; 16],
        identity: Option<[u8; 16]>,
        alias: Option<String>,
    },
    SetAlias {
        destination: [u8; 16],
        alias: Option<String>,
    },
    SetPinned {
        destination: [u8; 16],
        pinned: bool,
    },
    Delete {
        destination: [u8; 16],
    },
    Get {
        destination: [u8; 16],
    },
    List,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DirectoryResponse {
    Mutation(ContactMutationOutcome),
    Lookup(ContactLookupOutcome),
    List(ContactListOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactRecord {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    alias: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    identity: Option<[u8; 16]>,
    pinned: bool,
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub(crate) fn execute(database: &Database, request: DirectoryRequest) -> StoreReply {
    if let DirectoryRequest::CreateManual { alias, .. } | DirectoryRequest::SetAlias { alias, .. } =
        &request
    {
        if !crate::input::bounded_text(&[alias.as_deref().unwrap_or_default()]) {
            return Err(DevelopmentStoreFailure::unavailable(
                "the contact alias exceeds the native input size limit",
            ));
        }
    }
    match request {
        DirectoryRequest::SaveObserved {
            destination,
            identity,
        } => save_observed(database, destination, identity),
        DirectoryRequest::CreateManual {
            destination,
            identity,
            alias,
        } => create_manual(database, destination, identity, alias),
        DirectoryRequest::SetAlias { destination, alias } => {
            set_alias(database, destination, alias)
        }
        DirectoryRequest::SetPinned {
            destination,
            pinned,
        } => set_pinned(database, destination, pinned),
        DirectoryRequest::Delete { destination } => delete(database, destination),
        DirectoryRequest::Get { destination } => get(database, destination),
        DirectoryRequest::List => list(database),
    }
}

pub(crate) fn decode_contact(
    key: &[u8],
    encoded: &[u8],
) -> Result<Contact, DevelopmentStoreFailure> {
    let destination: [u8; 16] = key.try_into().map_err(|_| {
        DevelopmentStoreFailure::reset_required(format!(
            "a development contact key holds {} bytes instead of 16",
            key.len()
        ))
    })?;
    let record: ContactRecord = serde_json::from_slice(encoded).map_err(|error| {
        DevelopmentStoreFailure::reset_required(format!(
            "a development contact record is malformed: {error}"
        ))
    })?;
    if record.alias != normalize_alias(record.alias.clone()) {
        return Err(DevelopmentStoreFailure::reset_required(
            "a development contact alias is not in canonical form",
        ));
    }
    if record.pinned && record.identity.is_none() {
        return Err(DevelopmentStoreFailure::reset_required(
            "a pinned development contact has no identity",
        ));
    }
    Ok(contact(destination, record))
}

fn create_manual(
    database: &Database,
    destination: [u8; 16],
    identity: Option<[u8; 16]>,
    alias: Option<String>,
) -> StoreReply {
    let write = begin_write(database, "create manual contact")?;
    let outcome;
    {
        let mut table = open_contacts_write(&write)?;
        if let Some(encoded) = read_encoded(&table, &destination)? {
            outcome = ContactMutationOutcome::AlreadyExists {
                contact: decode_contact(&destination, &encoded)?,
            };
        } else {
            let record = ContactRecord {
                alias: normalize_alias(alias),
                identity,
                pinned: false,
            };
            insert_record(&mut table, &destination, &record)?;
            outcome = ContactMutationOutcome::Saved {
                contact: contact(destination, record),
            };
        }
    }
    if matches!(outcome, ContactMutationOutcome::Saved { .. }) {
        commit(write, "create manual contact")?;
    }
    Ok(DirectoryResponse::Mutation(outcome))
}

fn save_observed(database: &Database, destination: [u8; 16], identity: [u8; 16]) -> StoreReply {
    let write = begin_write(database, "save observed contact")?;
    let outcome;
    let mut changed = false;
    {
        let mut table = open_contacts_write(&write)?;
        if let Some(encoded) = read_encoded(&table, &destination)? {
            let mut existing = decode_contact(&destination, &encoded)?;
            outcome = match existing.identity {
                None => {
                    existing.identity = Some(identity);
                    let record = ContactRecord::from(&existing);
                    insert_record(&mut table, &destination, &record)?;
                    changed = true;
                    ContactMutationOutcome::Updated { contact: existing }
                }
                Some(existing_identity) if existing_identity == identity => {
                    ContactMutationOutcome::Existing { contact: existing }
                }
                Some(existing_identity) => ContactMutationOutcome::IdentityConflict {
                    existing: existing_identity,
                    attempted: identity,
                },
            };
        } else {
            let record = ContactRecord {
                alias: None,
                identity: Some(identity),
                pinned: false,
            };
            insert_record(&mut table, &destination, &record)?;
            changed = true;
            outcome = ContactMutationOutcome::Saved {
                contact: contact(destination, record),
            };
        }
    }
    if changed {
        commit(write, "save observed contact")?;
    }
    Ok(DirectoryResponse::Mutation(outcome))
}

fn set_alias(database: &Database, destination: [u8; 16], alias: Option<String>) -> StoreReply {
    let write = begin_write(database, "set contact alias")?;
    let outcome;
    {
        let mut table = open_contacts_write(&write)?;
        let Some(encoded) = read_encoded(&table, &destination)? else {
            return Ok(DirectoryResponse::Mutation(
                ContactMutationOutcome::NotFound,
            ));
        };
        let mut existing = decode_contact(&destination, &encoded)?;
        existing.alias = normalize_alias(alias);
        let record = ContactRecord::from(&existing);
        insert_record(&mut table, &destination, &record)?;
        outcome = ContactMutationOutcome::Updated { contact: existing };
    }
    commit(write, "set contact alias")?;
    Ok(DirectoryResponse::Mutation(outcome))
}

fn set_pinned(database: &Database, destination: [u8; 16], pinned: bool) -> StoreReply {
    let write = begin_write(database, "set contact pin")?;
    let outcome;
    {
        let mut table = open_contacts_write(&write)?;
        let Some(encoded) = read_encoded(&table, &destination)? else {
            return Ok(DirectoryResponse::Mutation(
                ContactMutationOutcome::NotFound,
            ));
        };
        let mut existing = decode_contact(&destination, &encoded)?;
        if pinned && existing.identity.is_none() {
            return Ok(DirectoryResponse::Mutation(
                ContactMutationOutcome::MissingIdentity,
            ));
        }
        existing.pinned = pinned;
        let record = ContactRecord::from(&existing);
        insert_record(&mut table, &destination, &record)?;
        outcome = ContactMutationOutcome::Updated { contact: existing };
    }
    commit(write, "set contact pin")?;
    Ok(DirectoryResponse::Mutation(outcome))
}

fn delete(database: &Database, destination: [u8; 16]) -> StoreReply {
    let write = begin_write(database, "delete contact")?;
    let deleted;
    {
        let mut table = open_contacts_write(&write)?;
        deleted = table
            .remove(destination.as_slice())
            .map_err(|error| classify_redb_error(error.into(), "delete contact record"))?
            .is_some();
    }
    if deleted {
        commit(write, "delete contact")?;
        Ok(DirectoryResponse::Mutation(ContactMutationOutcome::Deleted))
    } else {
        Ok(DirectoryResponse::Mutation(
            ContactMutationOutcome::NotFound,
        ))
    }
}

fn get(database: &Database, destination: [u8; 16]) -> StoreReply {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "get contact"))?;
    let table = read
        .open_table(CONTACTS)
        .map_err(|error| classify_redb_error(error.into(), "open development contacts table"))?;
    let outcome = match read_encoded(&table, &destination)? {
        Some(encoded) => ContactLookupOutcome::Found {
            contact: decode_contact(&destination, &encoded)?,
        },
        None => ContactLookupOutcome::NotFound,
    };
    Ok(DirectoryResponse::Lookup(outcome))
}

fn list(database: &Database) -> StoreReply {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "list contacts"))?;
    let table = read
        .open_table(CONTACTS)
        .map_err(|error| classify_redb_error(error.into(), "open development contacts table"))?;
    let mut contacts = Vec::new();
    let entries = table
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan development contacts table"))?;
    for entry in entries {
        let (key, value) = entry.map_err(|error| {
            classify_redb_error(error.into(), "scan development contact record")
        })?;
        contacts.push(decode_contact(key.value(), value.value())?);
    }
    Ok(DirectoryResponse::List(ContactListOutcome::Listed {
        contacts,
    }))
}

fn begin_write(
    database: &Database,
    operation: &str,
) -> Result<redb::WriteTransaction, DevelopmentStoreFailure> {
    database
        .begin_write()
        .map_err(|error| classify_redb_error(error.into(), operation))
}

fn open_contacts_write<'a>(
    write: &'a redb::WriteTransaction,
) -> Result<redb::Table<'a, &'static [u8], &'static [u8]>, DevelopmentStoreFailure> {
    write
        .open_table(CONTACTS)
        .map_err(|error| classify_redb_error(error.into(), "open development contacts table"))
}

fn commit(write: redb::WriteTransaction, operation: &str) -> Result<(), DevelopmentStoreFailure> {
    write
        .commit()
        .map_err(|error| classify_redb_error(error.into(), operation))
}

fn read_encoded(
    table: &impl ReadableTable<&'static [u8], &'static [u8]>,
    destination: &[u8; 16],
) -> Result<Option<Vec<u8>>, DevelopmentStoreFailure> {
    table
        .get(destination.as_slice())
        .map(|value| value.map(|value| value.value().to_vec()))
        .map_err(|error| classify_redb_error(error.into(), "read development contact record"))
}

fn insert_record(
    table: &mut redb::Table<'_, &'static [u8], &'static [u8]>,
    destination: &[u8; 16],
    record: &ContactRecord,
) -> Result<(), DevelopmentStoreFailure> {
    let encoded = serde_json::to_vec(record).map_err(|error| {
        DevelopmentStoreFailure::unavailable(format!(
            "could not encode development contact record: {error}"
        ))
    })?;
    table
        .insert(destination.as_slice(), encoded.as_slice())
        .map(|_| ())
        .map_err(|error| classify_redb_error(error.into(), "write development contact record"))
}

fn normalize_alias(alias: Option<String>) -> Option<String> {
    alias.and_then(|alias| {
        let trimmed = alias.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn contact(destination: [u8; 16], record: ContactRecord) -> Contact {
    Contact {
        destination,
        alias: record.alias,
        identity: record.identity,
        pinned: record.pinned,
    }
}

impl From<&Contact> for ContactRecord {
    fn from(contact: &Contact) -> Self {
        Self {
            alias: contact.alias.clone(),
            identity: contact.identity,
            pinned: contact.pinned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::development_store::DevelopmentStoreOwner;

    fn call(owner: &DevelopmentStoreOwner, request: DirectoryRequest) -> DirectoryResponse {
        owner
            .admit_directory_async(request)
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap()
    }

    #[test]
    fn contact_operations_follow_closed_semantics_and_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let first = [1; 16];
        let second = [2; 16];
        let first_identity = [11; 16];
        let second_identity = [22; 16];
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();

        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::CreateManual {
                    destination: second,
                    identity: None,
                    alias: Some("  Bob  ".to_owned()),
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Saved {
                contact: Contact {
                    alias: Some(ref alias),
                    pinned: false,
                    identity: None,
                    ..
                }
            }) if alias == "Bob"
        ));
        assert_eq!(
            call(
                &owner,
                DirectoryRequest::SetPinned {
                    destination: second,
                    pinned: true,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::MissingIdentity)
        );
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SaveObserved {
                    destination: second,
                    identity: second_identity,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated { .. })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SaveObserved {
                    destination: second,
                    identity: second_identity,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Existing { .. })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SetPinned {
                    destination: second,
                    pinned: true,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated {
                contact: Contact { pinned: true, .. }
            })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SetPinned {
                    destination: second,
                    pinned: true,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated {
                contact: Contact { pinned: true, .. }
            })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SetPinned {
                    destination: second,
                    pinned: false,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated {
                contact: Contact {
                    pinned: false,
                    identity: Some(identity),
                    ..
                }
            }) if identity == second_identity
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SetPinned {
                    destination: second,
                    pinned: true,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated { .. })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SaveObserved {
                    destination: second,
                    identity: first_identity,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::IdentityConflict {
                existing,
                attempted,
            }) if existing == second_identity && attempted == first_identity
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::CreateManual {
                    destination: first,
                    identity: Some(first_identity),
                    alias: None,
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Saved { .. })
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::CreateManual {
                    destination: first,
                    identity: Some(second_identity),
                    alias: Some("replacement".to_owned()),
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::AlreadyExists {
                contact: Contact {
                    identity: Some(identity),
                    alias: None,
                    ..
                }
            }) if identity == first_identity
        ));
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::SetAlias {
                    destination: first,
                    alias: Some("   ".to_owned()),
                }
            ),
            DirectoryResponse::Mutation(ContactMutationOutcome::Updated {
                contact: Contact { alias: None, .. }
            })
        ));
        assert!(matches!(
            call(&owner, DirectoryRequest::List),
            DirectoryResponse::List(ContactListOutcome::Listed { contacts })
                if contacts.iter().map(|contact| contact.destination).collect::<Vec<_>>()
                    == vec![first, second]
        ));
        owner.close().unwrap();

        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        assert!(matches!(
            call(&owner, DirectoryRequest::Get { destination: second }),
            DirectoryResponse::Lookup(ContactLookupOutcome::Found {
                contact: Contact {
                    alias: Some(ref alias),
                    identity: Some(identity),
                    pinned: true,
                    ..
                }
            }) if alias == "Bob" && identity == second_identity
        ));
        assert_eq!(
            call(&owner, DirectoryRequest::Delete { destination: first }),
            DirectoryResponse::Mutation(ContactMutationOutcome::Deleted)
        );
        assert_eq!(
            call(&owner, DirectoryRequest::Delete { destination: first }),
            DirectoryResponse::Mutation(ContactMutationOutcome::NotFound)
        );
        owner.close().unwrap();
    }

    #[test]
    fn record_codec_requires_nullable_fields_and_rejects_unknowns() {
        assert!(matches!(
            decode_contact(&[0; 16], br#"{"identity":null,"pinned":false}"#),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
        assert!(matches!(
            decode_contact(
                &[0; 16],
                br#"{"alias":null,"identity":null,"pinned":false,"extra":1}"#
            ),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
        assert!(matches!(
            decode_contact(
                &[0; 15],
                br#"{"alias":null,"identity":null,"pinned":false}"#
            ),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
        assert!(matches!(
            decode_contact(
                &[0; 16],
                br#"{"alias":" Alice ","identity":null,"pinned":false}"#
            ),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
        assert!(matches!(
            decode_contact(&[0; 16], br#"{"alias":null,"identity":null,"pinned":true}"#),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
    }
}
