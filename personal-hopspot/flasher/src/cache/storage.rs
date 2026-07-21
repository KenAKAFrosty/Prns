#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prns_flash_manifest::sha256_hex;

use super::{channel_name, read_limited, signature_path, CandidateError, VerifiedCandidate};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn publish(
    cache_root: &Path,
    candidate: &VerifiedCandidate,
) -> Result<(), CandidateError> {
    let releases = cache_root.join("releases");
    create_directory(&releases)?;
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

    let release_was_new = if existing_directory(&final_release)? {
        verify_existing_release(&final_release, candidate)?;
        false
    } else {
        fs::rename(&staging.path, &final_release).map_err(|source| CandidateError::Filesystem {
            action: "publish cache directory",
            path: final_release.clone(),
            source,
        })?;
        staging.keep();
        sync_directory(&releases)?;
        true
    };

    if let Err(error) = publish_channel(cache_root, candidate) {
        if release_was_new {
            fs::remove_dir_all(&final_release).map_err(|source| CandidateError::Filesystem {
                action: "roll back cache directory",
                path: final_release,
                source,
            })?;
            sync_directory(&releases)?;
        }
        return Err(error);
    }
    Ok(())
}

fn verify_existing_release(
    release: &Path,
    candidate: &VerifiedCandidate,
) -> Result<(), CandidateError> {
    compare_file(&release.join("flash-manifest.json"), &candidate.manifest)?;
    compare_file(
        &release.join("flash-manifest.json.minisig"),
        &candidate.manifest_signature,
    )?;
    for artifact in &candidate.artifacts {
        compare_file(
            &release.join(&artifact.board_slug).join(&artifact.file_name),
            &artifact.bytes,
        )?;
    }
    Ok(())
}

fn publish_channel(cache_root: &Path, candidate: &VerifiedCandidate) -> Result<(), CandidateError> {
    let directory = cache_root
        .join("channels")
        .join(channel_name(candidate.channel));
    let identifier = sha256_hex(&candidate.descriptor);
    let descriptor = directory.join(format!("{identifier}.json"));
    let signature = signature_path(&descriptor);
    let descriptor_existed = descriptor.is_file();
    store_immutable(&descriptor, &candidate.descriptor)?;
    if let Err(error) = store_immutable(&signature, &candidate.descriptor_signature) {
        if !descriptor_existed {
            let _ = fs::remove_file(&descriptor);
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn store_immutable(path: &Path, bytes: &[u8]) -> Result<(), CandidateError> {
    if path.exists() {
        return compare_file(path, bytes);
    }
    let parent = path.parent().ok_or_else(|| CandidateError::UnsafePath {
        path: path.display().to_string(),
    })?;
    create_directory(parent)?;
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
