#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prns_flash_manifest::{sha256_hex, BoardCatalog, ReleaseChannel, ValidatedFlashManifest};

use super::{
    channel_name, read_limited, signature_path, CandidateError, SignatureVerifier,
    VerifiedCandidate, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES,
};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
const MAX_CACHED_RELEASE_ENTRIES: usize = 10_000;
pub(super) const MAX_CACHED_RELEASE_DEPTH: usize = 64;

pub(super) fn publish(
    cache_root: &Path,
    candidate: &VerifiedCandidate,
    catalog: &BoardCatalog,
    trusted_public_key: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<(), CandidateError> {
    let releases = cache_root.join("releases");
    ensure_cache_directory(cache_root, &releases)?;
    let final_release = releases.join(&candidate.version);
    let mut staging = StagingDirectory::create(&releases, &candidate.version)?;
    write_new_file(
        &staging.path.join("flash-manifest.json"),
        &candidate.manifest,
    )?;
    write_new_file(
        &staging.path.join("flash-manifest.json.minisig"),
        &candidate.manifest_signature,
    )?;
    for artifact in &candidate.artifacts {
        write_new_file(
            &staging
                .path
                .join(&artifact.board_slug)
                .join(&artifact.file_name),
            &artifact.bytes,
        )?;
    }
    sync_directory_tree(&staging.path)?;

    if existing_directory(&final_release)? {
        match inspect_existing_release(
            &final_release,
            candidate,
            catalog,
            trusted_public_key,
            verifier,
        ) {
            ExistingRelease::Exact => {}
            ExistingRelease::ValidConflict => {
                return Err(CandidateError::ImmutableConflict {
                    path: final_release.join("flash-manifest.json"),
                });
            }
            ExistingRelease::Corrupt => {
                replace_directory(&releases, &final_release, &mut staging)?;
            }
        }
    } else {
        fs::rename(&staging.path, &final_release).map_err(|source| CandidateError::Filesystem {
            action: "publish cache directory",
            path: final_release.clone(),
            source,
        })?;
        staging.keep();
        sync_directory(&releases)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExistingRelease {
    Exact,
    ValidConflict,
    Corrupt,
}

fn inspect_existing_release(
    release: &Path,
    candidate: &VerifiedCandidate,
    catalog: &BoardCatalog,
    trusted_public_key: &str,
    verifier: &dyn SignatureVerifier,
) -> ExistingRelease {
    if !release_tree_is_regular(release) {
        return ExistingRelease::Corrupt;
    }
    let manifest_path = release.join("flash-manifest.json");
    let signature_path = release.join("flash-manifest.json.minisig");
    let manifest = read_limited(&manifest_path, MAX_MANIFEST_BYTES).ok();
    let signature = read_limited(&signature_path, MAX_SIGNATURE_BYTES).ok();

    if manifest.as_deref() == Some(candidate.manifest.as_slice())
        && signature.as_deref() == Some(candidate.manifest_signature.as_slice())
    {
        let artifacts_match = candidate.artifacts.iter().all(|artifact| {
            file_matches(
                &release.join(&artifact.board_slug).join(&artifact.file_name),
                &artifact.bytes,
            )
        });
        return if artifacts_match {
            ExistingRelease::Exact
        } else {
            ExistingRelease::Corrupt
        };
    }

    let existing_identity = manifest
        .as_deref()
        .zip(signature.as_deref())
        .and_then(|(manifest, signature)| {
            std::str::from_utf8(signature)
                .ok()
                .map(|signature| (manifest, signature))
        })
        .and_then(|(manifest, signature)| {
            verifier
                .verify(manifest, signature, trusted_public_key)
                .ok()
                .and_then(|()| ValidatedFlashManifest::from_json(manifest, catalog).ok())
        });
    if existing_identity.is_some_and(|manifest| {
        manifest.release().version().as_str() == candidate.version
            && manifest.release().channel() == candidate.channel
            && manifest.signing().key_id().as_str() == candidate.key_id
    }) {
        ExistingRelease::ValidConflict
    } else {
        ExistingRelease::Corrupt
    }
}

fn release_tree_is_regular(root: &Path) -> bool {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut entries_seen = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            return false;
        };
        for entry in entries {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_CACHED_RELEASE_ENTRIES {
                return false;
            }
            let Ok(entry) = entry else {
                return false;
            };
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            if file_type.is_symlink() {
                return false;
            }
            if file_type.is_dir() {
                if depth >= MAX_CACHED_RELEASE_DEPTH {
                    return false;
                }
                pending.push((entry.path(), depth + 1));
            } else if !file_type.is_file() {
                return false;
            }
        }
    }
    true
}

pub(super) fn publish_verified_channel(
    cache_root: &Path,
    channel: ReleaseChannel,
    descriptor_bytes: &[u8],
    signature_bytes: &[u8],
) -> Result<(), CandidateError> {
    let directory = cache_root.join("channels").join(channel_name(channel));
    ensure_cache_directory(cache_root, &directory)?;
    let identifier = sha256_hex(descriptor_bytes);
    let descriptor = directory.join(format!("{identifier}.json"));
    let signature = signature_path(&descriptor);
    store_verified(&descriptor, descriptor_bytes)?;
    store_verified(&signature, signature_bytes)?;
    publish_channel_head(&directory, &identifier)
}

pub(super) fn store_immutable(
    cache_root: &Path,
    path: &Path,
    bytes: &[u8],
) -> Result<(), CandidateError> {
    let parent = path.parent().ok_or_else(|| CandidateError::UnsafePath {
        path: path.display().to_string(),
    })?;
    ensure_cache_directory(cache_root, parent)?;
    if path.exists() {
        return compare_file(path, bytes);
    }
    let temporary = unique_temporary_file(parent, path.file_name())?;
    let result = (|| {
        write_new_file(&temporary, bytes)?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => sync_directory(parent),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                compare_file(path, bytes)
            }
            Err(source) => Err(CandidateError::Filesystem {
                action: "publish immutable cache file",
                path: path.to_path_buf(),
                source,
            }),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn compare_file(path: &Path, expected: &[u8]) -> Result<(), CandidateError> {
    let actual = read_limited(path, expected.len() as u64 + 1)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CandidateError::ImmutableConflict {
            path: path.to_path_buf(),
        })
    }
}

fn file_matches(path: &Path, expected: &[u8]) -> bool {
    read_limited(path, expected.len() as u64 + 1).is_ok_and(|actual| actual == expected)
}

fn store_verified(path: &Path, bytes: &[u8]) -> Result<(), CandidateError> {
    match fs::symlink_metadata(path) {
        Ok(_) if file_matches(path, bytes) => Ok(()),
        Ok(_) => atomic_replace_file(path, bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| CandidateError::UnsafePath {
                path: path.display().to_string(),
            })?;
            create_directory(parent)?;
            let temporary = unique_temporary_file(parent, path.file_name())?;
            let result = (|| {
                write_new_file(&temporary, bytes)?;
                fs::rename(&temporary, path).map_err(|source| CandidateError::Filesystem {
                    action: "publish verified cache file",
                    path: path.to_path_buf(),
                    source,
                })?;
                sync_directory(parent)
            })();
            let _ = fs::remove_file(&temporary);
            result
        }
        Err(source) => Err(CandidateError::Filesystem {
            action: "inspect",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn publish_channel_head(directory: &Path, identifier: &str) -> Result<(), CandidateError> {
    let bytes = format!("{identifier}\n");
    atomic_replace_file(&directory.join("HEAD"), bytes.as_bytes())
}

fn atomic_replace_file(path: &Path, bytes: &[u8]) -> Result<(), CandidateError> {
    let parent = path.parent().ok_or_else(|| CandidateError::UnsafePath {
        path: path.display().to_string(),
    })?;
    create_directory(parent)?;
    let temporary = unique_temporary_file(parent, path.file_name())?;
    let backup = unique_backup_entry(parent, path.file_name())?;
    write_new_file(&temporary, bytes)?;
    let had_existing = match fs::rename(path, &backup) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(source) => {
            let _ = fs::remove_file(&temporary);
            return Err(CandidateError::Filesystem {
                action: "move old cache file aside",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if let Err(source) = fs::rename(&temporary, path) {
        if had_existing {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(CandidateError::Filesystem {
            action: "publish verified cache file",
            path: path.to_path_buf(),
            source,
        });
    }
    sync_directory(parent)?;
    if had_existing {
        remove_entry(&backup)?;
        sync_directory(parent)?;
    }
    Ok(())
}

fn replace_directory(
    parent: &Path,
    destination: &Path,
    staging: &mut StagingDirectory,
) -> Result<(), CandidateError> {
    let backup = unique_backup_entry(parent, destination.file_name())?;
    fs::rename(destination, &backup).map_err(|source| CandidateError::Filesystem {
        action: "move corrupt cache directory aside",
        path: destination.to_path_buf(),
        source,
    })?;
    if let Err(source) = fs::rename(&staging.path, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(CandidateError::Filesystem {
            action: "publish repaired cache directory",
            path: destination.to_path_buf(),
            source,
        });
    }
    staging.keep();
    sync_directory(parent)?;
    remove_entry(&backup)?;
    sync_directory(parent)
}

fn remove_entry(path: &Path) -> Result<(), CandidateError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| CandidateError::Filesystem {
        action: "inspect replaced cache entry",
        path: path.to_path_buf(),
        source,
    })?;
    let result = if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| CandidateError::Filesystem {
        action: "remove replaced cache entry",
        path: path.to_path_buf(),
        source,
    })
}

struct StagingDirectory {
    path: PathBuf,
    remove_on_drop: bool,
}

impl StagingDirectory {
    fn create(parent: &Path, version: &str) -> Result<Self, CandidateError> {
        for _ in 0..100 {
            let identifier = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".import-{version}-{}-{identifier}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        remove_on_drop: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(CandidateError::Filesystem {
                        action: "create cache staging directory",
                        path,
                        source,
                    });
                }
            }
        }
        Err(CandidateError::Filesystem {
            action: "create unique cache staging directory",
            path: parent.to_path_buf(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "temporary name exhaustion"),
        })
    }

    fn keep(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn unique_temporary_file(
    parent: &Path,
    file_name: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, CandidateError> {
    let file_name =
        file_name
            .and_then(|name| name.to_str())
            .ok_or_else(|| CandidateError::UnsafePath {
                path: parent.display().to_string(),
            })?;
    for _ in 0..100 {
        let identifier = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.part-{}-{identifier}",
            std::process::id()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(CandidateError::Filesystem {
        action: "allocate temporary cache file",
        path: parent.to_path_buf(),
        source: io::Error::new(io::ErrorKind::AlreadyExists, "temporary name exhaustion"),
    })
}

fn unique_backup_entry(
    parent: &Path,
    file_name: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, CandidateError> {
    let file_name =
        file_name
            .and_then(|name| name.to_str())
            .ok_or_else(|| CandidateError::UnsafePath {
                path: parent.display().to_string(),
            })?;
    for _ in 0..100 {
        let identifier = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.replaced-{}-{identifier}",
            std::process::id()
        ));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Ok(_) => {}
            Err(source) => {
                return Err(CandidateError::Filesystem {
                    action: "inspect cache replacement backup",
                    path,
                    source,
                });
            }
        }
    }
    Err(CandidateError::Filesystem {
        action: "allocate cache replacement backup",
        path: parent.to_path_buf(),
        source: io::Error::new(io::ErrorKind::AlreadyExists, "backup name exhaustion"),
    })
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), CandidateError> {
    let parent = path.parent().ok_or_else(|| CandidateError::UnsafePath {
        path: path.display().to_string(),
    })?;
    create_directory(parent)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| CandidateError::Filesystem {
            action: "create",
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| CandidateError::Filesystem {
            action: "write and synchronize",
            path: path.to_path_buf(),
            source,
        })
}

fn ensure_cache_directory(cache_root: &Path, directory: &Path) -> Result<(), CandidateError> {
    let relative = directory
        .strip_prefix(cache_root)
        .map_err(|_| CandidateError::UnsafePath {
            path: directory.display().to_string(),
        })?;
    ensure_real_directory(cache_root, true)?;
    let mut current = cache_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(CandidateError::UnsafePath {
                path: directory.display().to_string(),
            });
        };
        current.push(component);
        ensure_real_directory(&current, false)?;
    }
    Ok(())
}

fn ensure_real_directory(path: &Path, create_parents: bool) -> Result<(), CandidateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            return Err(CandidateError::UnsafeEntry {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(CandidateError::Filesystem {
                action: "inspect cache directory",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let result = if create_parents {
        fs::create_dir_all(path)
    } else {
        fs::create_dir(path)
    };
    if let Err(source) = result {
        if source.kind() != io::ErrorKind::AlreadyExists {
            return Err(CandidateError::Filesystem {
                action: "create cache directory",
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|source| CandidateError::Filesystem {
        action: "inspect created cache directory",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(CandidateError::UnsafeEntry {
            path: path.to_path_buf(),
        })
    }
}

fn create_directory(path: &Path) -> Result<(), CandidateError> {
    fs::create_dir_all(path).map_err(|source| CandidateError::Filesystem {
        action: "create directory",
        path: path.to_path_buf(),
        source,
    })
}

fn existing_directory(path: &Path) -> Result<bool, CandidateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(CandidateError::UnsafeEntry {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CandidateError::Filesystem {
            action: "inspect",
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), CandidateError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| CandidateError::Filesystem {
            action: "synchronize directory",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), CandidateError> {
    Ok(())
}

fn sync_directory_tree(root: &Path) -> Result<(), CandidateError> {
    fn visit(directory: &Path) -> Result<(), CandidateError> {
        for entry in fs::read_dir(directory).map_err(|source| CandidateError::Filesystem {
            action: "inspect",
            path: directory.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| CandidateError::Filesystem {
                action: "inspect",
                path: directory.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|source| CandidateError::Filesystem {
                    action: "inspect",
                    path: path.clone(),
                    source,
                })?
                .is_dir()
            {
                visit(&path)?;
            }
        }
        sync_directory(directory)
    }

    visit(root)
}
