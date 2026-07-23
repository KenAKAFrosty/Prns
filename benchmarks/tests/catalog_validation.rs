use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use benchmarks::{
    load_all_rows, load_catalog, load_implementations, ScenarioTopology, Subject, IMPLEMENTATIONS,
    RESULT_SCHEMA_VERSION,
};
type HostScenario = (String, String);
type SubjectSample = (String, u32);

fn benchmark_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[test]
fn scenario_directories_and_manifests_are_unique_and_complete() {
    let mut slugs = BTreeSet::new();
    for entry in std::fs::read_dir(benchmark_dir().join("scenarios")).expect("scenario directory") {
        let path = entry.expect("scenario entry").path();
        if !path.is_dir() {
            continue;
        }
        let directory = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("UTF-8 scenario directory");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path.join("manifest.json")).expect("scenario manifest"),
        )
        .expect("valid scenario manifest");
        let slug = manifest["name"].as_str().expect("manifest name");
        assert_eq!(directory, slug, "manifest and directory disagree");
        assert!(
            slugs.insert(slug.to_string()),
            "duplicate scenario slug {slug}"
        );
        for field in ["title", "category", "summary", "primary_metric"] {
            assert!(
                !manifest[field].as_str().unwrap_or("").trim().is_empty(),
                "{slug} lacks {field}"
            );
        }
        assert!(
            manifest["headline"].is_boolean(),
            "{slug} lacks headline status"
        );
        assert!(manifest["notes"].is_array(), "{slug} lacks concise notes");
        assert!(
            manifest["cell_notes"].is_null() || manifest["cell_notes"].is_array(),
            "{slug} has malformed cell notes"
        );
    }
    assert_eq!(
        slugs,
        load_catalog()
            .expect("typed catalog")
            .into_iter()
            .map(|manifest| manifest.name.as_str().to_string())
            .collect(),
        "the public suite catalog and scenario directories must agree"
    );
}

#[test]
fn implementation_participant_contracts_are_executable_and_consistent() {
    let implementations = load_implementations();
    let mut slugs = BTreeSet::new();
    for implementation in implementations {
        assert!(
            slugs.insert(implementation.slug.clone()),
            "duplicate implementation slug"
        );
        if let Some(participant) = implementation.participant {
            assert!(
                !participant.command.is_empty(),
                "{} has an empty command",
                implementation.slug
            );
            assert!(
                !participant.command[0].trim().is_empty(),
                "{} has no executable",
                implementation.slug
            );
        }
    }
    assert_eq!(
        slugs,
        IMPLEMENTATIONS.into_iter().map(str::to_string).collect(),
        "only Prns and the compiled RNS 1.4.0 reference belong in this suite"
    );
}

#[test]
fn every_committed_result_is_schema_v2_and_matches_its_path() {
    let rows = load_all_rows();
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(row.schema_version, RESULT_SCHEMA_VERSION);
        assert!(!row.run_id.is_empty());
        assert!(benchmark_dir()
            .join("scenarios")
            .join(&row.scenario)
            .is_dir());
        let Subject::Direct {
            initiator,
            responder,
            relay,
        } = &row.subject;
        if let Some(relay) = relay {
            assert_eq!(initiator, "benchmark-wire-driver");
            assert_eq!(responder, "benchmark-wire-driver");
            assert!(IMPLEMENTATIONS.contains(&relay.as_str()));
        } else {
            assert!(IMPLEMENTATIONS.contains(&initiator.as_str()));
            assert!(IMPLEMENTATIONS.contains(&responder.as_str()));
        }
    }
    for jsonl in jsonl_files(&benchmark_dir().join("results")) {
        let filename = jsonl
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("result filename");
        let first = std::fs::read_to_string(&jsonl).expect("result file");
        let row: benchmarks::ResultRow =
            serde_json::from_str(first.lines().next().expect("result row")).expect("schema-v2 row");
        assert_eq!(
            filename,
            row.subject.file_slug(),
            "typed subject disagrees with {}",
            jsonl.display()
        );
        assert_eq!(
            jsonl
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            Some(row.scenario.as_str())
        );
    }
}

#[test]
fn every_published_scenario_has_its_complete_topology_matrix() {
    let mut cells: BTreeMap<HostScenario, BTreeSet<SubjectSample>> = BTreeMap::new();
    for row in load_all_rows() {
        let slug = row.subject.file_slug();
        cells
            .entry((row.host, row.scenario))
            .or_default()
            .insert((slug, row.sample_index));
    }
    assert!(!cells.is_empty(), "at least one host publishes results");
    let hosts = cells
        .keys()
        .map(|(host, _)| host.clone())
        .collect::<BTreeSet<_>>();
    for host in hosts {
        for manifest in load_catalog().expect("typed catalog") {
            let scenario = manifest.name.as_str().to_string();
            let observed = cells
                .get(&(host.clone(), scenario.clone()))
                .cloned()
                .unwrap_or_default();
            // Historical immutable suites may predate a newly added scenario. Once a
            // scenario is present, however, its topology must be complete.
            if observed.is_empty() {
                continue;
            }
            let subjects = match manifest.topology {
                ScenarioTopology::Direct => IMPLEMENTATIONS
                    .iter()
                    .flat_map(|initiator| {
                        IMPLEMENTATIONS
                            .iter()
                            .map(move |responder| Subject::Direct {
                                initiator: (*initiator).into(),
                                responder: (*responder).into(),
                                relay: None,
                            })
                    })
                    .collect::<Vec<_>>(),
                ScenarioTopology::Relay => IMPLEMENTATIONS
                    .iter()
                    .map(|relay| Subject::Direct {
                        initiator: "benchmark-wire-driver".into(),
                        responder: "benchmark-wire-driver".into(),
                        relay: Some((*relay).into()),
                    })
                    .collect::<Vec<_>>(),
            };
            let expected = subjects
                .into_iter()
                .flat_map(|subject| {
                    let slug = subject.file_slug();
                    (0..3).map(move |sample| (slug.clone(), sample))
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(
                observed, expected,
                "{host}/{scenario} must publish its complete topology with three samples"
            );
        }
    }
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(jsonl_files(&path));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files
}

#[test]
fn typed_subjects_round_trip() {
    for subject in [
        Subject::Direct {
            initiator: "a".into(),
            responder: "b".into(),
            relay: None,
        },
        Subject::Direct {
            initiator: "benchmark-wire-driver".into(),
            responder: "benchmark-wire-driver".into(),
            relay: Some("personal-rns".into()),
        },
    ] {
        let json = serde_json::to_string(&subject).expect("serialize subject");
        assert_eq!(
            serde_json::from_str::<Subject>(&json).expect("deserialize subject"),
            subject
        );
    }
}
