use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy)]
pub struct RepositoryDocument {
    pub slug: &'static str,
    pub title: &'static str,
    pub source_path: &'static str,
    pub route: &'static str,
    pub source: &'static str,
}

pub const GUIDE_DOCUMENTS: &[RepositoryDocument] = &[
    RepositoryDocument {
        slug: "getting-started",
        title: "Getting Started",
        source_path: "docs/getting-started.md",
        route: "/guides/getting-started",
        source: include_str!("../../getting-started.md"),
    },
    RepositoryDocument {
        slug: "testing",
        title: "Testing Changes",
        source_path: "docs/testing.md",
        route: "/guides/testing",
        source: include_str!("../../testing.md"),
    },
    RepositoryDocument {
        slug: "embedded",
        title: "Embedded Prns",
        source_path: "docs/embedded.md",
        route: "/guides/embedded",
        source: include_str!("../../embedded.md"),
    },
    RepositoryDocument {
        slug: "personal-hopspot",
        title: "Personal Hopspot",
        source_path: "personal-hopspot/README.md",
        route: "/guides/personal-hopspot",
        source: include_str!("../../../personal-hopspot/README.md"),
    },
    RepositoryDocument {
        slug: "benchmarks",
        title: "Benchmarking Prns",
        source_path: "benchmarks/README.md",
        route: "/guides/benchmarks",
        source: include_str!("../../../benchmarks/README.md"),
    },
    RepositoryDocument {
        slug: "configuration",
        title: "Prnsd Configuration",
        source_path: "docs/prnsd-config.md",
        route: "/guides/configuration",
        source: include_str!("../../prnsd-config.md"),
    },
    RepositoryDocument {
        slug: "utilities",
        title: "Prnsd Utilities",
        source_path: "docs/prnsd-utilities.md",
        route: "/guides/utilities",
        source: include_str!("../../prnsd-utilities.md"),
    },
    RepositoryDocument {
        slug: "validation",
        title: "Validation",
        source_path: "docs/validation.md",
        route: "/guides/validation",
        source: include_str!("../../validation.md"),
    },
    RepositoryDocument {
        slug: "observability",
        title: "Observability",
        source_path: "docs/observability.md",
        route: "/guides/observability",
        source: include_str!("../../observability.md"),
    },
    RepositoryDocument {
        slug: "profiling",
        title: "Benchmark Profiling",
        source_path: "benchmarks/PROFILING.md",
        route: "/guides/profiling",
        source: include_str!("../../../benchmarks/PROFILING.md"),
    },
    RepositoryDocument {
        slug: "benchmark-methodology",
        title: "Benchmark Qualification",
        source_path: "benchmarks/CONTRIBUTING.md",
        route: "/guides/benchmark-methodology",
        source: include_str!("../../../benchmarks/CONTRIBUTING.md"),
    },
    RepositoryDocument {
        slug: "repository-tools",
        title: "Repository Tools",
        source_path: "tools/README.md",
        route: "/guides/repository-tools",
        source: include_str!("../../../tools/README.md"),
    },
    RepositoryDocument {
        slug: "website",
        title: "Website Development",
        source_path: "docs/website/README.md",
        route: "/guides/website",
        source: include_str!("../README.md"),
    },
    RepositoryDocument {
        slug: "release",
        title: "Release Guidance",
        source_path: "docs/release.md",
        route: "/guides/release",
        source: include_str!("../../release.md"),
    },
];

const ROUTE_MAPPINGS: &[(&str, &str)] = &[
    ("README.md", "/"),
    ("CONTRIBUTING.md", "/contributing"),
    ("prnsd/README.md", "/crates/prnsd"),
    ("personal-rns/README.md", "/crates/personal-rns"),
    ("personal-hopspot/README.md", "/guides/personal-hopspot"),
    ("docs/getting-started.md", "/guides/getting-started"),
    ("docs/testing.md", "/guides/testing"),
    ("docs/embedded.md", "/guides/embedded"),
    ("benchmarks/README.md", "/guides/benchmarks"),
    ("docs/prnsd-config.md", "/guides/configuration"),
    ("docs/prnsd-utilities.md", "/guides/utilities"),
    ("docs/validation.md", "/guides/validation"),
    ("docs/observability.md", "/guides/observability"),
    ("benchmarks/PROFILING.md", "/guides/profiling"),
    (
        "benchmarks/CONTRIBUTING.md",
        "/guides/benchmark-methodology",
    ),
    ("tools/README.md", "/guides/repository-tools"),
    ("docs/website/README.md", "/guides/website"),
    ("docs/release.md", "/guides/release"),
];

pub fn guide(slug: &str) -> Option<&'static RepositoryDocument> {
    GUIDE_DOCUMENTS
        .iter()
        .find(|document| document.slug == slug)
}

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
    let route = ROUTE_MAPPINGS
        .iter()
        .find_map(|(source, route)| (*source == normalized).then_some(*route))
        .ok_or_else(|| {
            format!("{source_path} links to unmounted repository Markdown {target} ({normalized})")
        })?;
    let mut resolved = route.to_string();
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

    #[test]
    fn guide_registry_has_unique_slugs_routes_and_sources() {
        let mut slugs = HashSet::new();
        let mut routes = HashSet::new();
        for document in GUIDE_DOCUMENTS {
            assert!(
                slugs.insert(document.slug),
                "duplicate slug {}",
                document.slug
            );
            assert!(
                routes.insert(document.route),
                "duplicate route {}",
                document.route
            );
            assert!(
                !document.source.trim().is_empty(),
                "missing {}",
                document.source_path
            );
            assert_eq!(
                guide(document.slug).map(|found| found.route),
                Some(document.route)
            );
        }
    }

    #[test]
    fn every_mounted_document_has_resolved_markdown_links() {
        for document in GUIDE_DOCUMENTS {
            repository_markup(document.source_path, document.source, true)
                .unwrap_or_else(|error| panic!("{error}"));
        }
        repository_markup(
            "CONTRIBUTING.md",
            include_str!("../../../CONTRIBUTING.md"),
            true,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for (path, source) in [
            ("prnsd/README.md", include_str!("../../../prnsd/README.md")),
            (
                "personal-rns/README.md",
                include_str!("../../../personal-rns/README.md"),
            ),
        ] {
            repository_markup(path, source, true).unwrap_or_else(|error| panic!("{error}"));
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
        assert!(markup.contains("](/guides/configuration#daemon-behavior)"));
    }

    #[test]
    fn unresolved_relative_markdown_fails_closed() {
        let error = repository_markup(
            "docs/testing.md",
            "[missing](not-a-mounted-guide.md)",
            false,
        )
        .expect_err("unmounted Markdown must fail");
        assert!(error.contains("unmounted repository Markdown"));
    }

    #[test]
    fn every_resolved_repository_target_is_a_live_site_route() {
        let guide_routes = GUIDE_DOCUMENTS
            .iter()
            .map(|document| document.route)
            .collect::<HashSet<_>>();
        for (_, route) in ROUTE_MAPPINGS {
            assert!(
                matches!(
                    *route,
                    "/" | "/contributing" | "/crates/prnsd" | "/crates/personal-rns"
                ) || guide_routes.contains(route),
                "dead local route {route}"
            );
        }
    }

    #[test]
    fn mounted_onboarding_uses_clone_valid_commands() {
        let sources = GUIDE_DOCUMENTS
            .iter()
            .map(|document| document.source)
            .chain([
                include_str!("../../../CONTRIBUTING.md"),
                include_str!("../../../prnsd/README.md"),
                include_str!("../../../personal-rns/README.md"),
            ])
            .collect::<Vec<_>>();
        assert!(sources
            .iter()
            .any(|source| source.contains("cargo tools guide rust")));
        assert!(sources
            .iter()
            .all(|source| !source.contains("cargo add prnsd")
                && !source.contains("cargo add personal-rns")));
    }
}
