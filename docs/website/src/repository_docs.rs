use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum GuideSection {
    Start,
    Operate,
    Contribute,
    Maintain,
}

#[derive(Clone, Copy)]
pub struct RepositoryDocument {
    pub title: &'static str,
    pub summary: &'static str,
    pub section: GuideSection,
    pub source_path: &'static str,
}

pub const REPOSITORY_BLOB_BASE: &str = "https://github.com/KenAKAFrosty/Prns/blob/main";

impl RepositoryDocument {
    pub fn github_url(&self) -> String {
        format!("{REPOSITORY_BLOB_BASE}/{}", self.source_path)
    }
}

pub const GUIDE_DOCUMENTS: &[RepositoryDocument] = &[
    RepositoryDocument {
        title: "Run and inspect a node",
        summary:
            "Start an isolated prnsd node, inspect its interfaces, attach to logs, and stop it cleanly.",
        section: GuideSection::Start,
        source_path: "prnsd/README.md",
    },
    RepositoryDocument {
        title: "Build a Rust application",
        summary:
            "Run two real nodes, then learn the recipe, handles, events, features, and API contract.",
        section: GuideSection::Start,
        source_path: "personal-rns/README.md",
    },
    RepositoryDocument {
        title: "Getting Started",
        summary: "Run a node, try the Rust API, test a change, and measure performance.",
        section: GuideSection::Start,
        source_path: "docs/getting-started.md",
    },
    RepositoryDocument {
        title: "More Key Concepts",
        summary:
            "Learn the working vocabulary, from packets and routes to channels, daemons, and LXMF.",
        section: GuideSection::Start,
        source_path: "docs/more-concepts.md",
    },
    RepositoryDocument {
        title: "Testing Changes",
        summary: "Choose the right verification rung and report useful evidence.",
        section: GuideSection::Start,
        source_path: "docs/testing.md",
    },
    RepositoryDocument {
        title: "Example Catalog",
        summary: "Choose runnable, compile-checked, browser-hosted, or illustrative examples.",
        section: GuideSection::Start,
        source_path: "docs/examples.md",
    },
    RepositoryDocument {
        title: "Coming from RNS",
        summary: "Carry your config and apps over from rnsd and see what you gain.",
        section: GuideSection::Start,
        source_path: "docs/coming-from-rns.md",
    },
    RepositoryDocument {
        title: "Embedded Prns",
        summary: "Build a real board target and follow the node recipe into hardware.",
        section: GuideSection::Start,
        source_path: "docs/embedded.md",
    },
    RepositoryDocument {
        title: "Personal Hopspot",
        summary: "Understand the board-backed node application across supported platforms.",
        section: GuideSection::Operate,
        source_path: "personal-hopspot/README.md",
    },
    RepositoryDocument {
        title: "Deploy Prnsd",
        summary: "Run the production container, operate hosted pages, and deploy through Railway.",
        section: GuideSection::Operate,
        source_path: "docs/deploy-prnsd.md",
    },
    RepositoryDocument {
        title: "Benchmarking Prns",
        summary: "Run smoke or full measurements and interpret qualified results.",
        section: GuideSection::Start,
        source_path: "benchmarks/README.md",
    },
    RepositoryDocument {
        title: "Prnsd Configuration",
        summary: "Configure interfaces, policy, discovery, and managed live changes.",
        section: GuideSection::Operate,
        source_path: "docs/prnsd-config.md",
    },
    RepositoryDocument {
        title: "Prnsd Utilities",
        summary: "Use status, path, probe, identity, copy, and execution commands.",
        section: GuideSection::Operate,
        source_path: "docs/prnsd-utilities.md",
    },
    RepositoryDocument {
        title: "Validation",
        summary: "Understand suite registration, evidence, and release aggregation.",
        section: GuideSection::Maintain,
        source_path: "docs/validation.md",
    },
    RepositoryDocument {
        title: "Observability",
        summary: "Operate lifecycle logs, structured events, metrics, and traces.",
        section: GuideSection::Operate,
        source_path: "docs/observability.md",
    },
    RepositoryDocument {
        title: "Benchmark Profiling",
        summary: "Use focused microscopes and platform profilers on measured hot paths.",
        section: GuideSection::Contribute,
        source_path: "benchmarks/PROFILING.md",
    },
    RepositoryDocument {
        title: "Benchmark Qualification",
        summary: "Apply the methodology and publication rules behind performance claims.",
        section: GuideSection::Maintain,
        source_path: "benchmarks/CONTRIBUTING.md",
    },
    RepositoryDocument {
        title: "Repository Tools",
        summary: "Discover supported build, device, release, and repository operations.",
        section: GuideSection::Contribute,
        source_path: "tools/README.md",
    },
    RepositoryDocument {
        title: "Website Development",
        summary: "Run and test the site locally with the Dioxus toolchain.",
        section: GuideSection::Contribute,
        source_path: "docs/website/README.md",
    },
    RepositoryDocument {
        title: "Release Guidance",
        summary: "Build, validate, sign, and publish through the release-custody path.",
        section: GuideSection::Maintain,
        source_path: "docs/release.md",
    },
];

const ROUTE_MAPPINGS: &[(&str, &str)] = &[("CONTRIBUTING.md", "/contributing")];

pub fn repository_markup(
    source_path: &str,
    source: &str,
    strip_heading: bool,
) -> Result<String, String> {
    let source = if strip_heading {
        strip_first_heading(source)
    } else {
        source
    };
    rewrite_relative_links(source_path, source)
}

fn strip_first_heading(source: &str) -> &str {
    let Some(first_line_end) = source.find('\n') else {
        return if source.starts_with("# ") { "" } else { source };
    };
    if !source.starts_with("# ") {
        return source;
    }
    source[first_line_end + 1..].trim_start_matches('\n')
}

fn rewrite_relative_links(source_path: &str, source: &str) -> Result<String, String> {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(open) = rest.find("](") {
        let target_start = open + 2;
        output.push_str(&rest[..target_start]);
        let candidate = &rest[target_start..];
        let Some(close) = candidate.find(')') else {
            output.push_str(candidate);
            return Ok(output);
        };
        let target = &candidate[..close];
        output.push_str(&resolve_target(source_path, target)?);
        output.push(')');
        rest = &candidate[close + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn resolve_target(source_path: &str, target: &str) -> Result<String, String> {
    if target.starts_with('#')
        || target.starts_with('/')
        || target.contains("://")
        || target.starts_with("mailto:")
    {
        return Ok(target.to_string());
    }
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    if !path.ends_with(".md") {
        return Ok(target.to_string());
    }
    let normalized = normalize_relative(source_path, path)?;
    let mut resolved = ROUTE_MAPPINGS
        .iter()
        .find_map(|(source, route)| (*source == normalized).then(|| (*route).to_string()))
        .unwrap_or_else(|| format!("{REPOSITORY_BLOB_BASE}/{normalized}"));
    if let Some(fragment) = fragment {
        resolved.push('#');
        resolved.push_str(fragment);
    }
    Ok(resolved)
}

fn normalize_relative(source_path: &str, target: &str) -> Result<String, String> {
    let parent = Path::new(source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut normalized = PathBuf::new();
    for component in parent.join(target).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!(
                        "{source_path} links outside the repository: {target}"
                    ));
                }
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "{source_path} has an invalid relative link: {target}"
                ));
            }
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const MOUNTED_SOURCES: &[(&str, &str)] =
        &[("CONTRIBUTING.md", include_str!("../../../CONTRIBUTING.md"))];

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn markdown_targets(source: &str) -> Vec<&str> {
        let mut targets = Vec::new();
        let mut rest = source;
        while let Some(open) = rest.find("](") {
            let candidate = &rest[open + 2..];
            let Some(close) = candidate.find(')') else {
                break;
            };
            targets.push(&candidate[..close]);
            rest = &candidate[close + 1..];
        }
        targets
    }

    #[test]
    fn the_guide_directory_is_complete_and_sectioned() {
        let mut source_paths = HashSet::new();
        let mut sections = HashSet::new();
        for document in GUIDE_DOCUMENTS {
            assert!(
                source_paths.insert(document.source_path),
                "duplicate source {}",
                document.source_path
            );
            assert!(
                !document.title.trim().is_empty() && !document.summary.trim().is_empty(),
                "missing copy for {}",
                document.source_path
            );
            assert!(document.github_url().starts_with(REPOSITORY_BLOB_BASE));
            sections.insert(document.section);
        }
        assert_eq!(
            sections.len(),
            4,
            "every guide section must contain a guide"
        );
    }

    #[test]
    fn the_guide_directory_offers_the_quickstart_and_the_catalog() {
        for expected in ["docs/getting-started.md", "docs/examples.md"] {
            assert!(
                GUIDE_DOCUMENTS
                    .iter()
                    .any(|document| document.source_path == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn every_guide_source_path_exists_in_the_repository() {
        for document in GUIDE_DOCUMENTS {
            assert!(
                repository_root().join(document.source_path).is_file(),
                "{} is listed but absent from the tree",
                document.source_path
            );
        }
    }

    #[test]
    fn every_mounted_document_has_resolved_markdown_links() {
        for (path, source) in MOUNTED_SOURCES {
            repository_markup(path, source, true).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    #[test]
    fn relative_markdown_links_in_mounted_documents_point_at_real_files() {
        for (source_path, source) in MOUNTED_SOURCES {
            for target in markdown_targets(source) {
                if target.starts_with('#')
                    || target.starts_with('/')
                    || target.contains("://")
                    || target.starts_with("mailto:")
                {
                    continue;
                }
                let path = target.split('#').next().unwrap_or(target);
                if !path.ends_with(".md") {
                    continue;
                }
                let normalized =
                    normalize_relative(source_path, path).unwrap_or_else(|error| panic!("{error}"));
                assert!(
                    repository_root().join(&normalized).is_file(),
                    "{source_path} links to missing repository Markdown {target} ({normalized})"
                );
            }
        }
    }

    #[test]
    fn resolver_strips_one_heading_and_preserves_fragments() {
        let markup = repository_markup(
            "docs/testing.md",
            "# Page\n\nSee [configuration](prnsd-config.md#daemon-behavior).\n",
            true,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(!markup.starts_with("# Page"));
        assert!(markup
            .contains("](https://github.com/KenAKAFrosty/Prns/blob/main/docs/prnsd-config.md#daemon-behavior)"));
    }

    #[test]
    fn mounted_markdown_resolves_to_site_routes() {
        let markup = repository_markup("README.md", "[contribute](CONTRIBUTING.md)", false)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(markup, "[contribute](/contributing)");
    }

    #[test]
    fn unmounted_markdown_resolves_to_github_and_keeps_fragments() {
        let markup = repository_markup(
            "docs/coming-from-rns.md",
            "[the SDK list](../README.md#what-is-prns)",
            false,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            markup,
            "[the SDK list](https://github.com/KenAKAFrosty/Prns/blob/main/README.md#what-is-prns)"
        );
    }

    #[test]
    fn links_outside_the_repository_fail_closed() {
        let error = repository_markup("README.md", "[escape](../secret.md)", false)
            .expect_err("links outside the repository must fail");
        assert!(error.contains("outside the repository"));
    }

    #[test]
    fn every_resolved_repository_target_is_a_live_site_route() {
        for (_, route) in ROUTE_MAPPINGS {
            assert!(
                matches!(*route, "/contributing"),
                "dead local route {route}"
            );
        }
    }
}
