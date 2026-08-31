use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use reticulum_site::seo::{canonical_url, indexed_route_paths, static_route_paths};

fn main() -> ExitCode {
    match run() {
        Ok(page_count) => {
            println!("validated {page_count} Dioxus SSG pages; wrote 404.html and sitemap.xml");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("SSG finalization failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<usize, Box<dyn std::error::Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| invalid("usage: finalize_ssg <Dioxus public directory>"))?;
    let routes = static_route_paths();
    let indexed = indexed_route_paths().into_iter().collect::<HashSet<_>>();

    for route in &routes {
        let page = route_file(&output, route)?;
        let html = fs::read_to_string(&page)?;
        validate_page(route, &html, indexed.contains(route))?;
    }

    let not_found = route_file(&output, "/404")?;
    fs::copy(not_found, output.join("404.html"))?;
    fs::write(output.join("sitemap.xml"), sitemap(&indexed))?;

    Ok(routes.len())
}

fn route_file(output: &Path, route: &str) -> Result<PathBuf, io::Error> {
    if !route.starts_with('/') || route.split('/').any(|segment| segment == "..") {
        return Err(invalid(format!("unsafe static route: {route}")));
    }

    Ok(if route == "/" {
        output.join("index.html")
    } else {
        output
            .join(route.trim_start_matches('/'))
            .join("index.html")
    })
}

fn validate_page(route: &str, html: &str, indexed: bool) -> Result<(), io::Error> {
    require(
        route,
        html.starts_with("<!DOCTYPE html>"),
        "a leading HTML doctype",
    )?;
    require(
        route,
        html.matches("<title>").count() == 1,
        "exactly one title",
    )?;
    require(
        route,
        html.matches("name=\"description\"").count() == 1,
        "exactly one description",
    )?;
    require(route, html.matches("<h1").count() == 1, "exactly one h1")?;
    require(
        route,
        html.matches("id=\"main\"").count() == 1,
        "one app mount",
    )?;
    require(
        route,
        html.contains("window.initial_dioxus_hydration_data="),
        "Dioxus hydration data",
    )?;

    if indexed {
        let canonical = format!("rel=\"canonical\" href=\"{}\"", canonical_url(route));
        require(
            route,
            html.contains(&canonical),
            "the expected canonical URL",
        )?;
        require(
            route,
            !html.contains("name=\"robots\" content=\"noindex\""),
            "no noindex tag",
        )?;
    } else {
        require(
            route,
            html.contains("name=\"robots\" content=\"noindex\""),
            "a noindex tag",
        )?;
        require(
            route,
            !html.contains("rel=\"canonical\""),
            "no canonical URL",
        )?;
    }

    Ok(())
}

fn require(route: &str, condition: bool, expectation: &str) -> Result<(), io::Error> {
    if condition {
        Ok(())
    } else {
        Err(invalid(format!("{route} must contain {expectation}")))
    }
}

fn sitemap(indexed: &HashSet<String>) -> String {
    let mut routes = indexed.iter().collect::<Vec<_>>();
    routes.sort();

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for route in routes {
        xml.push_str("  <url><loc>");
        xml.push_str(&escape_xml(&canonical_url(route)));
        xml.push_str("</loc></url>\n");
    }
    xml.push_str("</urlset>\n");
    xml
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_files_follow_the_dioxus_ssg_directory_layout() {
        let output = Path::new("public");
        assert_eq!(route_file(output, "/").unwrap(), output.join("index.html"));
        assert_eq!(
            route_file(output, "/flash/heltec-v4").unwrap(),
            output.join("flash/heltec-v4/index.html")
        );
        assert!(route_file(output, "/../secret").is_err());
    }

    #[test]
    fn sitemap_escapes_urls_and_sorts_them() {
        let indexed = ["/z".to_string(), "/a&b".to_string()].into_iter().collect();
        let xml = sitemap(&indexed);

        assert!(xml.find("/a&amp;b").unwrap() < xml.find("/z").unwrap());
    }
}
