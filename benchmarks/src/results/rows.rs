use std::path::{Path, PathBuf};
use std::{fs::File, io::Write};

use super::results_dir;
use super::schema::ResultRow;

#[derive(serde::Deserialize)]
struct CurrentSuite {
    schema: u32,
    path: PathBuf,
}

pub fn write_rows(host: &str, scenario: &str, impl_slug: &str, rows: &[ResultRow]) {
    write_rows_at(&results_dir(), host, scenario, impl_slug, rows);
}

fn write_rows_at(root: &Path, host: &str, scenario: &str, impl_slug: &str, rows: &[ResultRow]) {
    assert!(!rows.is_empty(), "refuse to write an empty result set");
    let run_id = &rows[0].run_id;
    let subject = rows[0].subject.file_slug();
    assert_eq!(subject, impl_slug, "filename must match the typed subject");
    assert!(
        rows.iter().all(|row| {
            row.schema_version == super::schema::RESULT_SCHEMA_VERSION
                && &row.run_id == run_id
                && row.scenario == scenario
                && row.host == host
                && row.subject.file_slug() == impl_slug
        }),
        "all rows in one write must describe the same schema-v2 run and subject"
    );

    let dir = root.join(host).join(scenario);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let path = dir.join(format!("{impl_slug}.jsonl"));
    let mut complete = if path.exists() {
        let existing = load_rows_from(&path);
        if existing.first().is_some_and(|row| row.run_id == *run_id) {
            existing
                .into_iter()
                .filter(|row| row.sample_index != rows[0].sample_index)
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    complete.extend_from_slice(rows);
    complete.sort_by_key(|row| (row.sample_index, row.axis.order(), row.metric.clone()));

    let mut body = String::new();
    for row in &complete {
        body.push_str(&serde_json::to_string(row).expect("serialize result row"));
        body.push('\n');
    }
    let temp = dir.join(format!(
        ".{impl_slug}.{}.{}.tmp",
        std::process::id(),
        rows[0].sample_index
    ));
    let mut file = File::create(&temp)
        .unwrap_or_else(|e| panic!("create staged result {}: {e}", temp.display()));
    file.write_all(body.as_bytes())
        .unwrap_or_else(|e| panic!("write staged result {}: {e}", temp.display()));
    file.sync_all()
        .unwrap_or_else(|e| panic!("sync staged result {}: {e}", temp.display()));
    commit_temp(&temp, &path)
        .unwrap_or_else(|e| panic!("commit {} to {}: {e}", temp.display(), path.display()));
    #[cfg(unix)]
    File::open(&dir)
        .and_then(|directory| directory.sync_all())
        .unwrap_or_else(|e| panic!("sync result directory {}: {e}", dir.display()));
}

#[cfg(not(windows))]
fn commit_temp(temp: &Path, path: &Path) -> std::io::Result<()> {
    // POSIX rename replaces the destination atomically, so a crash can expose either the
    // previous complete result or the new complete result, never a partially written file.
    std::fs::rename(temp, path)
}

#[cfg(windows)]
fn commit_temp(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    if !path.exists() {
        return std::fs::rename(temp, path);
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }

    let replaced: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let replacement: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let success = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            1, // REPLACEFILE_WRITE_THROUGH
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if success != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub fn load_all_rows() -> Vec<ResultRow> {
    let mut rows = Vec::new();
    for jsonl in selected_jsonl_files(&results_dir()) {
        rows.extend(load_rows_from(&jsonl));
    }
    rows
}

/// Select one immutable suite per published host. A local run has no `current.json`, so its
/// host/scenario tree remains directly readable by the same renderer.
fn selected_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let host = entry.path();
        if !host.is_dir() || host.file_name().and_then(|name| name.to_str()) == Some("logs") {
            continue;
        }
        let pointer = host.join("current.json");
        if pointer.is_file() {
            let current: CurrentSuite = serde_json::from_str(
                &std::fs::read_to_string(&pointer)
                    .unwrap_or_else(|error| panic!("read {}: {error}", pointer.display())),
            )
            .unwrap_or_else(|error| panic!("parse {}: {error}", pointer.display()));
            assert_eq!(current.schema, 1, "unsupported current-suite schema");
            assert!(
                !current.path.is_absolute()
                    && current
                        .path
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_))),
                "current-suite path must stay beneath its host directory"
            );
            files.extend(jsonl_files(&host.join(current.path)));
        } else {
            files.extend(jsonl_files(&host));
        }
    }
    files.sort();
    files
}

fn load_rows_from(jsonl: &Path) -> Vec<ResultRow> {
    let text =
        std::fs::read_to_string(jsonl).unwrap_or_else(|e| panic!("read {}: {e}", jsonl.display()));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("parse row in {}: {e}", jsonl.display()))
        })
        .collect()
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(jsonl_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::schema::{Axis, ResultRow, Subject, RESULT_SCHEMA_VERSION};
    use std::collections::BTreeMap;

    fn row(run_id: &str, sample_index: u32, value: f64) -> ResultRow {
        ResultRow {
            schema_version: RESULT_SCHEMA_VERSION,
            run_id: run_id.into(),
            sample_index,
            scenario: "scenario".into(),
            scenario_version: 1,
            subject: Subject::Direct {
                initiator: "a".into(),
                responder: "b".into(),
                relay: None,
            },
            commit: "commit".into(),
            toolchain: "toolchain".into(),
            host: "host".into(),
            axis: Axis::Conformance,
            metric: "settled_clean".into(),
            value: Some(value),
            unit: "bool".into(),
            device_id: None,
            submitter_id: None,
            provenance: BTreeMap::new(),
        }
    }

    #[test]
    fn a_rejected_write_cannot_replace_the_last_valid_file() {
        let root =
            std::env::temp_dir().join(format!("benchmark-row-test-{}", uuid::Uuid::new_v4()));
        write_rows_at(&root, "host", "scenario", "a--b", &[row("valid", 0, 1.0)]);
        let path = root.join("host/scenario/a--b.jsonl");
        let before = std::fs::read_to_string(&path).expect("valid result");
        let rejected = std::panic::catch_unwind(|| {
            write_rows_at(
                &root,
                "host",
                "scenario",
                "wrong-subject",
                &[row("failed", 0, 0.0)],
            );
        });
        assert!(rejected.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("preserved result"),
            before
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
