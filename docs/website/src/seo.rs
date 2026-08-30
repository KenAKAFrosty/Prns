use std::collections::HashSet;

use dioxus::prelude::*;

use crate::pages::HOST_PAGES;
use crate::platforms::{all_board_targets, board_target_by_slug, BoardTarget, Tier};
use crate::routes::Route;

pub const CANONICAL_ORIGIN: &str = "https://reticulum.rs";

pub(crate) struct PageHead {
    title: String,
    description: String,
    indexed: bool,
}

#[component]
pub(crate) fn PageMetadata(route: Route) -> Element {
    let head = page_head(&route);
    let canonical = canonical_url(&route.to_string());

    // Dioxus 0.7 document Meta and Link components intentionally only apply
    // their initial props. Keep the SSR components below as the source of the
    // generated head, then synchronize those fields after client navigation.
    #[cfg(feature = "web")]
    use_effect(use_reactive!(|route| sync_client_head(&route)));

    rsx! {
        document::Title { "{head.title}" }
        document::Meta { name: "description", content: head.description.clone() }
        if head.indexed {
            document::Link { rel: "canonical", href: canonical.clone() }
        } else {
            document::Meta { name: "robots", content: "noindex" }
        }
        document::Meta { property: "og:title", content: head.title.clone() }
        document::Meta { property: "og:description", content: head.description.clone() }
        document::Meta { property: "og:url", content: canonical }
        document::Meta { name: "twitter:title", content: head.title }
        document::Meta { name: "twitter:description", content: head.description }
    }
}

#[cfg(feature = "web")]
fn sync_client_head(route: &Route) {
    let head = page_head(route);
    let metadata = serde_json::json!({
        "title": head.title,
        "description": head.description,
        "canonical": canonical_url(&route.to_string()),
        "indexed": head.indexed,
    });
    let script = format!(
        r#"
        requestAnimationFrame(() => {{
            const data = {metadata};
            const upsert = (selector, tag, attributes) => {{
                const matches = Array.from(document.head.querySelectorAll(selector));
                let element = matches.shift();
                for (const duplicate of matches) duplicate.remove();
                if (!element) {{
                    element = document.createElement(tag);
                    document.head.appendChild(element);
                }}
                for (const [name, value] of Object.entries(attributes)) {{
                    element.setAttribute(name, value);
                }}
            }};

            document.title = data.title;
            upsert('meta[name="description"]', 'meta', {{ name: 'description', content: data.description }});
            upsert('meta[property="og:title"]', 'meta', {{ property: 'og:title', content: data.title }});
            upsert('meta[property="og:description"]', 'meta', {{ property: 'og:description', content: data.description }});
            upsert('meta[property="og:url"]', 'meta', {{ property: 'og:url', content: data.canonical }});
            upsert('meta[name="twitter:title"]', 'meta', {{ name: 'twitter:title', content: data.title }});
            upsert('meta[name="twitter:description"]', 'meta', {{ name: 'twitter:description', content: data.description }});

            if (data.indexed) {{
                document.head.querySelector('meta[name="robots"]')?.remove();
                upsert('link[rel="canonical"]', 'link', {{ rel: 'canonical', href: data.canonical }});
            }} else {{
                document.head.querySelector('link[rel="canonical"]')?.remove();
                upsert('meta[name="robots"]', 'meta', {{ name: 'robots', content: 'noindex' }});
            }}
        }});
        "#
    );
    document::eval(&script);
}

pub(crate) fn page_head(route: &Route) -> PageHead {
    match route {
        Route::Landing {} => indexed(
            "Prns: High-performance Reticulum, built to run on any device",
            "A high-performance Rust port of Reticulum (RNS). A deterministic, no_std, alloc-free core, from a five-dollar microcontroller to a cloud server.",
        ),
        Route::PlatformsPage {} => indexed(
            "Where Prns runs its Reticulum engine",
            "One engine, many homes: Linux, macOS, Windows, Android, iOS, ESP32 and nRF52 microcontrollers, browsers, Node, and more, with Hopspot board support listed separately.",
        ),
        Route::FlashPage {} => indexed(
            "Flash a Personal Reticulum Hopspot | Prns",
            "Choose your exact board and flash a signed Prns release straight from your browser. Every byte is verified locally before it touches the device.",
        ),
        Route::FlashBoardPage { board } => match board_target_by_slug(board) {
            Some(target) => board_head(target),
            None => noindex(
                "Board not found | Prns",
                "Choose one of the supported Personal Hopspot boards.",
            ),
        },
        Route::BenchmarksPage {} => indexed(
            "Benchmarked Reticulum performance in the open | Prns",
            "Every number comes from published results in the repo, measured on real hardware by a harness you can run yourself.",
        ),
        Route::BenchmarksHostPage { host } => indexed(
            format!("Prns Reticulum benchmarks: {host}"),
            format!("Full Prns Reticulum benchmark result tables measured on {host}."),
        ),
        Route::NotFound { .. } => noindex(
            "Page not found | Prns",
            "There's nothing at this address.",
        ),
    }
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub fn static_route_paths() -> Vec<String> {
    let mut routes = Route::static_routes()
        .into_iter()
        .map(|route| route.to_string())
        .collect::<Vec<_>>();
    routes.extend(all_board_targets().map(|board| {
        Route::FlashBoardPage {
            board: board.slug.to_string(),
        }
        .to_string()
    }));
    routes.extend(HOST_PAGES.iter().map(|(host, _)| {
        Route::BenchmarksHostPage {
            host: host.to_string(),
        }
        .to_string()
    }));
    routes.push("/404".to_string());

    let mut seen = HashSet::new();
    routes.retain(|route| seen.insert(route.clone()));
    routes
}

pub fn indexed_route_paths() -> Vec<String> {
    static_route_paths()
        .into_iter()
        .filter(|path| {
            path.parse::<Route>()
                .is_ok_and(|route| page_head(&route).indexed)
        })
        .collect()
}

pub fn canonical_url(route_path: &str) -> String {
    match route_path {
        "/" => format!("{CANONICAL_ORIGIN}/"),
        _ => format!("{CANONICAL_ORIGIN}{route_path}"),
    }
}

fn indexed(title: impl Into<String>, description: impl Into<String>) -> PageHead {
    PageHead {
        title: title.into(),
        description: description.into(),
        indexed: true,
    }
}

fn noindex(title: impl Into<String>, description: impl Into<String>) -> PageHead {
    PageHead {
        title: title.into(),
        description: description.into(),
        indexed: false,
    }
}

fn board_head(board: &'static BoardTarget) -> PageHead {
    if board.is_flashable() {
        return indexed(
            format!("Flash the {} | Prns", board.name),
            format!(
                "Flash a signed Prns release onto the {} ({}) straight from your browser. Every byte is verified locally before it touches the device.",
                board.name, board.silicon
            ),
        );
    }

    let status = match board.tier {
        Tier::Shipping => "shipping",
        Tier::SdkPreview => "SDK preview",
        Tier::Flashable => "flashable",
        Tier::Qualification => "hardware qualification",
        Tier::BringUp => "active bring-up",
        Tier::Roadmap => "roadmap",
    };
    noindex(
        format!("{} support status | Prns", board.name),
        format!(
            "Track Prns {status} status for the {} ({}). Browser flashing is not publicly available for this board yet.",
            board.name, board.silicon
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_routes_include_fixed_and_catalog_routes_once() {
        let routes = static_route_paths();
        let unique = routes.iter().collect::<HashSet<_>>();

        assert_eq!(routes.len(), unique.len());
        assert!(routes.iter().any(|route| route == "/"));
        assert!(routes.iter().any(|route| route == "/platforms"));
        assert!(routes.iter().any(|route| route == "/flash/heltec-v4"));
        assert!(routes
            .iter()
            .any(|route| route == "/benchmarks/aarch64-apple-darwin"));
        assert!(routes.iter().any(|route| route == "/404"));
    }

    #[test]
    fn unavailable_boards_are_honest_and_not_indexed() {
        let route = Route::FlashBoardPage {
            board: "bq-nano-g2-ultra".to_string(),
        };
        let head = page_head(&route);

        assert!(!head.indexed);
        assert!(head.title.contains("support status"));
        assert!(!head.title.starts_with("Flash the"));
        assert!(head.description.contains("not publicly available"));
    }

    #[test]
    fn every_indexed_route_has_a_canonical_url() {
        for route in indexed_route_paths() {
            assert!(canonical_url(&route).starts_with(CANONICAL_ORIGIN));
        }
    }
}
