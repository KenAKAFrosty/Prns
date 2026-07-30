use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use flate2::write::GzEncoder;
use flate2::Compression;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_SOURCE_DIGEST");
    println!("cargo:rerun-if-env-changed=PRNS_SOURCE_SHA256");
    track_git_head();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let build_commit_short = git_commit_short();
    let build_version = env::var("PRNS_BUILD_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string("../../../VERSION")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    let build_source_digest = env::var("PRNS_BUILD_SOURCE_DIGEST")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("PRNS_SOURCE_SHA256")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| build_commit_short.clone());
    println!("cargo:rustc-env=HOPSPOT_BUILD_COMMIT_SHORT={build_commit_short}");
    println!("cargo:rustc-env=HOPSPOT_BUILD_VERSION={build_version}");
    println!("cargo:rustc-env=HOPSPOT_BUILD_SOURCE_DIGEST={build_source_digest}");
    println!(
        "cargo:rustc-env=HOPSPOT_BUILD_IDENTITY=version={build_version} source={build_source_digest}"
    );

    // Only the ESP32-S3 (xtensa) overrides the linker's memory layout. The C6 (riscv32) takes
    // esp-hal's bundled esp32c6 memory.x; a generically-named package-root memory.x would shadow it
    // via the linker's CWD search, so the S3's is memory-esp32s3.x, copied to OUT_DIR for xtensa only.
    if target_arch == "xtensa" {
        // app/memory.x overrides esp-hal's bundled esp32s3 memory.x: the linker's `INCLUDE memory.x`
        // (from esp-hal's linkall.x) resolves it from the package root, ahead of esp-hal's copy. It
        // raises ORIGIN(dram2_seg) so the core-0 construction stack grows into the reclaimed heap
        // region — needed for the full Wi-Fi + LoRa + Bluetooth LE coexistence firmware, harmless to
        // Wi-Fi-only and Bluetooth LE-only builds (no Bluetooth reserve or Wi-Fi controller leaves
        // them DRAM to spare). Copied to
        // OUT_DIR + put on the link search path as the explicit mechanism; rerun-if-changed relinks
        // when memory.x is edited.
        fs::copy("memory-esp32s3.x", out.join("memory.x")).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rerun-if-changed=memory-esp32s3.x");
    }

    if env::var_os("CARGO_FEATURE_WIFI_AUTO").is_some() {
        generate_hopspot_site(&out, &build_commit_short);
    }
}

fn generate_hopspot_site(out: &std::path::Path, build_commit_short: &str) {
    let site_dir = env::var_os("HOPSPOT_SITE_PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("../../../docs/website/target/dx/reticulum-site/release/web/public")
        });
    let dest = out.join("hopspot_site.rs");
    println!("cargo:rerun-if-env-changed=HOPSPOT_SITE_PUBLIC");
    println!("cargo:rerun-if-env-changed=HOPSPOT_ALLOW_FALLBACK_SITE");
    println!("cargo:rerun-if-changed={}", site_dir.display());

    let Ok(site_dir) = site_dir.canonicalize() else {
        if allow_fallback_site() {
            fs::write(&dest, fallback_site_source()).unwrap();
            return;
        }
        panic!(
            "Hopspot SoftAP website bundle missing at {}. Run `cargo run -p hopspot-flash -- build heltec-v4` or build docs/website with PRNS_EMBEDDED_SITE=1 first.",
            site_dir.display()
        );
    };

    let mut files = Vec::new();
    collect_site_files(&site_dir, &site_dir, &mut files);
    prune_hosted_only_assets(&mut files);
    prune_stale_dioxus_assets(&site_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    if files.is_empty() {
        if allow_fallback_site() {
            fs::write(&dest, fallback_site_source()).unwrap();
            return;
        }
        panic!(
            "Hopspot SoftAP website bundle at {} did not contain embeddable files.",
            site_dir.display()
        );
    }
    let prepared_dir = out.join("hopspot_site_assets");
    let _ = fs::remove_dir_all(&prepared_dir);
    fs::create_dir_all(&prepared_dir).unwrap();

    let mut source = String::new();
    source.push_str(
        "pub struct SiteAsset {\n    pub path: &'static str,\n    pub content_type: &'static str,\n    pub bytes: &'static [u8],\n    pub gzip_bytes: Option<&'static [u8]>,\n}\n\n",
    );
    source.push_str("pub static SITE_ASSETS: &[SiteAsset] = &[\n");
    for (index, (path, file)) in files.into_iter().enumerate() {
        println!("cargo:rerun-if-changed={}", file.display());
        let content_type = content_type_for(&path);
        let file = prepare_site_file(&path, &file, &prepared_dir, build_commit_short);
        let gzip_file = if should_gzip_asset(&path, content_type) {
            gzip_site_file(&path, &file, &prepared_dir, index)
        } else {
            None
        };

        source.push_str("    SiteAsset { path: ");
        source.push_str(&format!("{path:?}"));
        source.push_str(", content_type: ");
        source.push_str(&format!("{content_type:?}"));
        source.push_str(", bytes: include_bytes!(r#\"");
        source.push_str(&file.display().to_string());
        source.push_str("\"#), gzip_bytes: ");
        if let Some(gzip_file) = gzip_file {
            source.push_str("Some(include_bytes!(r#\"");
            source.push_str(&gzip_file.display().to_string());
            source.push_str("\"#))");
        } else {
            source.push_str("None");
        }
        source.push_str(" },\n");
    }
    source.push_str("];\n");
    fs::write(dest, source).unwrap();
}

fn allow_fallback_site() -> bool {
    env::var("HOPSPOT_ALLOW_FALLBACK_SITE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn collect_site_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, PathBuf)>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_site_files(root, &path, out);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let mut web_path = String::from("/");
        web_path.push_str(&rel.to_string_lossy().replace('\\', "/"));
        out.push((web_path, path));
    }
}

fn prune_hosted_only_assets(files: &mut Vec<(String, PathBuf)>) {
    files.retain(|(path, _)| {
        path == "/index.html"
            || path == "/assets/tailwind.css"
            || path == "/assets/prns-mark.svg"
            || path == "/assets/favicon.svg"
            || is_dioxus_hashed_js(path)
            || is_dioxus_hashed_wasm(path)
    });
}

fn prune_stale_dioxus_assets(site_dir: &std::path::Path, files: &mut Vec<(String, PathBuf)>) {
    let index = fs::read_to_string(site_dir.join("index.html")).unwrap_or_default();
    let mut current_js = Vec::new();
    files.retain(|(path, file)| {
        if !is_dioxus_hashed_js(path) {
            return true;
        }
        let keep = path_leaf(path).is_some_and(|leaf| index.contains(leaf));
        if keep {
            current_js.push(file.clone());
        }
        keep
    });

    let mut js_bundle = String::new();
    for file in current_js {
        if let Ok(js) = fs::read_to_string(file) {
            js_bundle.push_str(&js);
        }
    }
    files.retain(|(path, _)| {
        !is_dioxus_hashed_wasm(path) || path_leaf(path).is_some_and(|leaf| js_bundle.contains(leaf))
    });
}

fn is_dioxus_hashed_js(path: &str) -> bool {
    path.starts_with("/assets/reticulum-site-dxh") && path.ends_with(".js")
}

fn is_dioxus_hashed_wasm(path: &str) -> bool {
    path.starts_with("/assets/reticulum-site_bg-dxh") && path.ends_with(".wasm")
}

fn path_leaf(path: &str) -> Option<&str> {
    path.rsplit('/').next().filter(|leaf| !leaf.is_empty())
}

fn prepare_site_file(
    path: &str,
    file: &std::path::Path,
    out: &std::path::Path,
    build_commit_short: &str,
) -> PathBuf {
    if path != "/index.html" {
        return file.to_owned();
    }
    let Ok(html) = fs::read_to_string(file) else {
        return file.to_owned();
    };
    let dest = out.join("index.html");
    fs::write(&dest, inject_hopspot_loader(&html, build_commit_short)).unwrap();
    dest
}

fn inject_hopspot_loader(html: &str, build_commit_short: &str) -> String {
    if html.contains("hopspot-loading") {
        return html.to_owned();
    }

    let mut out = html.to_owned();
    let style = r#"<style>
#hopspot-loading{position:fixed;inset:0;z-index:2147483647;display:grid;place-items:center;background:#071014;color:#eef6f7;font:500 16px system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
#hopspot-loading .panel{width:min(320px,calc(100vw - 48px))}
#hopspot-loading .brand{font-size:18px;font-weight:700;letter-spacing:0}
#hopspot-loading .hint{margin-top:8px;color:#9ab0b7;font-size:13px}
#hopspot-loading .meta{margin-top:8px;color:#6f858c;font:12px ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
#hopspot-loading .bar{height:3px;margin-top:18px;overflow:hidden;background:#1d3036;border-radius:999px}
#hopspot-loading .bar::before{content:"";display:block;width:45%;height:100%;background:#49d2a9;animation:hopspot-loader 1.15s ease-in-out infinite}
@keyframes hopspot-loader{0%{transform:translateX(-110%)}100%{transform:translateX(230%)}}
</style>
"#;
    if let Some(head_end) = out.find("</head>") {
        out.insert_str(head_end, style);
    }

    let marker = r#"<div id="main"></div>"#;
    let build_commit_short = html_attr_escape(build_commit_short);
    let loader = format!(
        r#"<div id="hopspot-loading" role="status" aria-live="polite"><div class="panel"><div class="brand">Loading Hopspot &amp; Prns Docs</div><div class="hint">This may take a little bit on the very first load.</div><div class="meta">Build {build_commit_short}</div><div class="bar"></div></div></div><div id="main"></div><script>(()=>{{const loader=document.getElementById("hopspot-loading");const main=document.getElementById("main");if(!loader||!main)return;const done=()=>{{loader.remove();observer.disconnect()}};const observer=new MutationObserver(()=>{{if(main.childNodes.length)done()}});observer.observe(main,{{childList:true}});}})();</script>"#
    );
    if let Some(main) = out.find(marker) {
        out.replace_range(main..main + marker.len(), &loader);
    }
    out
}

fn html_attr_escape(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn should_gzip_asset(path: &str, content_type: &str) -> bool {
    if content_type == "application/wasm" {
        return false;
    }

    content_type.starts_with("text/")
        || content_type == "application/json"
        || path.ends_with(".svg")
}

fn gzip_site_file(
    path: &str,
    file: &std::path::Path,
    out: &std::path::Path,
    index: usize,
) -> Option<PathBuf> {
    let bytes = fs::read(file).ok()?;
    if bytes.len() < 512 {
        return None;
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&bytes).ok()?;
    let compressed = encoder.finish().ok()?;
    if compressed.len() + 32 >= bytes.len() {
        return None;
    }

    let mut filename = format!("{index:03}");
    for ch in path.chars() {
        if ch.is_ascii_alphanumeric() {
            filename.push(ch);
        } else {
            filename.push('_');
        }
    }
    filename.push_str(".gz");

    let dest = out.join(filename);
    fs::write(&dest, compressed).ok()?;
    Some(dest)
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "sha256" => "text/plain; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn git_commit_short() -> String {
    env::var("PRNS_BUILD_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("PRNS_BUILD_COMMIT")
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(12).collect())
        })
        .or_else(|| git_output(&["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn track_git_head() {
    let mut paths = Vec::new();
    if let Some(path) = git_path("HEAD") {
        paths.push(path);
    }
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_path(&reference) {
            paths.push(path);
        }
        if let Some(path) = git_path("packed-refs") {
            paths.push(path);
        }
    }
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_path(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(git_output(&["rev-parse", "--git-path", name])?);
    if path.is_absolute() {
        return Some(path);
    }
    git_output(&["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map(|root| root.join(path))
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn fallback_site_source() -> &'static str {
    r#"pub struct SiteAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
    pub gzip_bytes: Option<&'static [u8]>,
}

pub static SITE_ASSETS: &[SiteAsset] = &[
    SiteAsset {
        path: "/index.html",
        content_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Hopspot</title><h1>Hopspot</h1><p>Website bundle not built into this firmware.</p>",
        gzip_bytes: None,
    },
];
"#
}
