use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flate2::write::GzEncoder;
use flate2::Compression;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Only the ESP32-S3 (xtensa) overrides the linker's memory layout. The C6 (riscv32) takes
    // esp-hal's bundled esp32c6 memory.x; a generically-named package-root memory.x would shadow it
    // via the linker's CWD search, so the S3's is memory-esp32s3.x, copied to OUT_DIR for xtensa only.
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("xtensa") {
        return;
    }
    // app/memory.x overrides esp-hal's bundled esp32s3 memory.x: the linker's `INCLUDE memory.x`
    // (from esp-hal's linkall.x) resolves it from the package root, ahead of esp-hal's copy. It
    // raises ORIGIN(dram2_seg) so the core-0 construction stack grows into the reclaimed heap
    // region — needed for the full WiFi+LoRa+BLE coex firmware, harmless to the WiFi-only and
    // BLE-only builds (no BT reserve / no WiFi controller leaves them DRAM to spare). Copied to
    // OUT_DIR + put on the link search path as the explicit mechanism; rerun-if-changed relinks
    // when memory.x is edited.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory-esp32s3.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-esp32s3.x");

    if env::var_os("CARGO_FEATURE_SOFTAP").is_some() {
        generate_hopspot_site(&out);
    }
}

fn generate_hopspot_site(out: &std::path::Path) {
    let site_dir = env::var_os("HOPSPOT_SITE_PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("../../docs/website/target/dx/reticulum-site/release/web/public")
        });
    let dest = out.join("hopspot_site.rs");
    println!("cargo:rerun-if-env-changed=HOPSPOT_SITE_PUBLIC");
    println!("cargo:rerun-if-changed={}", site_dir.display());

    let Ok(site_dir) = site_dir.canonicalize() else {
        fs::write(&dest, fallback_site_source()).unwrap();
        return;
    };

    let mut files = Vec::new();
    collect_site_files(&site_dir, &site_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    if files.is_empty() {
        fs::write(&dest, fallback_site_source()).unwrap();
        return;
    }

    let prepared_dir = out.join("hopspot_site_assets");
    fs::create_dir_all(&prepared_dir).unwrap();

    let mut source = String::new();
    source.push_str(
        "pub struct SiteAsset {\n    pub path: &'static str,\n    pub content_type: &'static str,\n    pub bytes: &'static [u8],\n    pub gzip_bytes: Option<&'static [u8]>,\n}\n\n",
    );
    source.push_str("pub static SITE_ASSETS: &[SiteAsset] = &[\n");
    for (index, (path, file)) in files.into_iter().enumerate() {
        println!("cargo:rerun-if-changed={}", file.display());
        let content_type = content_type_for(&path);
        let file = prepare_site_file(&path, &file, &prepared_dir);
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

fn prepare_site_file(path: &str, file: &std::path::Path, out: &std::path::Path) -> PathBuf {
    if path != "/index.html" {
        return file.to_owned();
    }
    let Ok(html) = fs::read_to_string(file) else {
        return file.to_owned();
    };
    let dest = out.join("index.html");
    fs::write(&dest, inject_hopspot_loader(&html)).unwrap();
    dest
}

fn inject_hopspot_loader(html: &str) -> String {
    if html.contains("hopspot-loading") {
        return html.to_owned();
    }

    let mut out = html.to_owned();
    let style = r#"<style>
#hopspot-loading{position:fixed;inset:0;z-index:2147483647;display:grid;place-items:center;background:#071014;color:#eef6f7;font:500 16px system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
#hopspot-loading .panel{width:min(320px,calc(100vw - 48px))}
#hopspot-loading .brand{font-size:18px;font-weight:700;letter-spacing:0}
#hopspot-loading .hint{margin-top:8px;color:#9ab0b7;font-size:13px}
#hopspot-loading .bar{height:3px;margin-top:18px;overflow:hidden;background:#1d3036;border-radius:999px}
#hopspot-loading .bar::before{content:"";display:block;width:45%;height:100%;background:#49d2a9;animation:hopspot-loader 1.15s ease-in-out infinite}
@keyframes hopspot-loader{0%{transform:translateX(-110%)}100%{transform:translateX(230%)}}
</style>
"#;
    if let Some(head_end) = out.find("</head>") {
        out.insert_str(head_end, style);
    }

    let marker = r#"<div id="main"></div>"#;
    let loader = r#"<div id="hopspot-loading" role="status" aria-live="polite"><div class="panel"><div class="brand">Loading Hopspot &amp; Prns Docs</div><div class="hint">This may take a little bit on the very first load.</div><div class="bar"></div></div></div><div id="main"></div><script>(()=>{const loader=document.getElementById("hopspot-loading");const main=document.getElementById("main");if(!loader||!main)return;const done=()=>{loader.remove();observer.disconnect()};const observer=new MutationObserver(()=>{if(main.childNodes.length)done()});observer.observe(main,{childList:true});})();</script>"#;
    if let Some(main) = out.find(marker) {
        out.replace_range(main..main + marker.len(), loader);
    }
    out
}

fn should_gzip_asset(path: &str, content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type == "application/wasm"
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
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
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
