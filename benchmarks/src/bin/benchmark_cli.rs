use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::Serialize;
use sha2::{Digest as _, Sha256};

const USAGE: &str = "usage: cargo benchmark [--smoke] [--publish] [--energy] [--resume RUN_ID]";
const HELP: &str = "Prns benchmark qualification\n\n  cargo benchmark             run all 20 cells × 3 isolated samples locally\n  cargo benchmark --smoke     check every direction quickly, without publishable data\n  cargo benchmark --resume ID continue a compatible interrupted local suite\n  cargo benchmark --publish   require a clean tree, run, then publish atomically\n  cargo benchmark --energy    require platform energy evidence\n\nRust/Cargo, uv, and a native C compiler are required. Generated local runs are ignored.";

#[derive(Default)]
struct Options {
    smoke: bool,
    publish: bool,
    energy: bool,
    resume: Option<String>,
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    MissingTool {
        tool: &'static str,
        setup: &'static str,
    },
    Command {
        purpose: &'static str,
        status: ExitStatus,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    Evidence(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(reason) => write!(formatter, "{reason}\n{USAGE}"),
            Self::MissingTool { tool, setup } => {
                write!(formatter, "missing prerequisite `{tool}`; {setup}")
            }
            Self::Command { purpose, status } => write!(formatter, "{purpose} exited {status}"),
            Self::Io {
                action,
                path,
                source,
            } => {
                write!(formatter, "{action} {}: {source}", path.display())
            }
            Self::Evidence(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for CliError {}

#[derive(serde::Deserialize, serde::Serialize)]
struct CompletedSuite {
    schema: u32,
    suite_id: String,
    source_commit: String,
    source_fingerprint: String,
    source_dirty: bool,
    samples_per_cell: u32,
    duration_ms: u64,
    selected_cells: usize,
    matrix_cells: usize,
    complete: bool,
    host: Option<String>,
    energy_available: bool,
    reference_verified: bool,
    failures: Vec<String>,
}

struct SourceState {
    commit: String,
    fingerprint: String,
}

#[derive(Serialize)]
struct CurrentSuite<'a> {
    schema: u32,
    suite_id: &'a str,
    source_commit: &'a str,
    path: String,
}

fn main() {
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "-h" || argument == "--help")
    {
        println!("{HELP}");
        return;
    }
    if let Err(error) = run() {
        eprintln!("BENCHMARK_ERROR: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CliError> {
    let options = parse_options(std::env::args().skip(1))?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let suite_id = options
        .resume
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid::Uuid::parse_str(&suite_id)
        .map_err(|_| CliError::Usage(format!("run ID must be a UUID, received {suite_id:?}")))?;
    let run_dir = root.join(".benchmark-runs").join(&suite_id);
    let source = source_state()?;

    if options.resume.is_some() {
        validate_resume(&run_dir, &suite_id, &source)?;
    }

    println!("Prns benchmark qualification");
    println!("  suite       {suite_id}");
    println!("  matrix      5 scenarios × 4 directions");
    println!(
        "  sampling    {}",
        if options.smoke {
            "one 500 ms smoke sample per cell"
        } else {
            "three counterbalanced 30 s samples per cell (~30–45 minutes)"
        }
    );
    println!(
        "  output      {} ({})",
        run_dir.display(),
        if options.publish {
            "publish after complete"
        } else {
            "local and gitignored"
        }
    );
    println!("  pass rule   every cell must run and satisfy exact scenario accounting");

    preflight(&options)?;
    if options.publish {
        require_clean_source()?;
    }
    build_harness(root)?;
    prepare_reference(root)?;
    std::fs::create_dir_all(&run_dir).map_err(|source| CliError::Io {
        action: "create run directory",
        path: run_dir.clone(),
        source,
    })?;
    describe_host(root, &run_dir)?;

    let mut runner = Command::new(binary(root, "benchmark_runner"));
    runner
        .arg("suite")
        .arg("release")
        .arg("--output")
        .arg(&run_dir)
        .arg("--suite-id")
        .arg(&suite_id)
        .env("BENCHMARK_BUILD_FLAGS", benchmark_rustflags())
        .env("BENCHMARK_SOURCE_FINGERPRINT", &source.fingerprint);
    if options.smoke {
        runner.arg("--smoke");
    }
    if options.energy {
        runner.env("BENCHMARK_REQUIRE_ENERGY", "1");
        #[cfg(target_os = "macos")]
        runner.env("BENCHMARK_POWER_VIA_SUDO", "1");
    }
    if let Err(error) = checked(&mut runner, "benchmark matrix") {
        eprintln!(
            "Resume with: cargo benchmark --resume {suite_id}{}{}",
            if options.publish { " --publish" } else { "" },
            if options.energy { " --energy" } else { "" }
        );
        return Err(error);
    }

    let suite = read_suite(&run_dir)?;
    validate_completed_suite(&suite, &options)?;
    if !options.smoke {
        render(root, &run_dir)?;
    }
    if options.publish {
        require_clean_source()?;
        promote(root, &run_dir, &suite)?;
        render(root, &root.join("results"))?;
        println!(
            "PUBLISHED suite={} host={} source={}",
            suite.suite_id,
            suite.host.as_deref().unwrap_or("unknown"),
            suite.source_commit
        );
    } else {
        println!("LOCAL_REPORT {}", run_dir.join("RESULTS.md").display());
        println!(
            "The checkout was not modified. Maintainers publish with `cargo benchmark --publish`."
        );
    }
    if !suite.energy_available {
        print_energy_follow_up();
    }
    Ok(())
}

fn parse_options(arguments: impl Iterator<Item = String>) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--smoke" => options.smoke = true,
            "--publish" => options.publish = true,
            "--energy" => options.energy = true,
            "--resume" => {
                options.resume = Some(
                    arguments
                        .next()
                        .ok_or_else(|| CliError::Usage("--resume needs a run ID".into()))?,
                );
            }
            "-h" | "--help" => return Err(CliError::Usage(String::new())),
            other => return Err(CliError::Usage(format!("unknown option {other}"))),
        }
    }
    if options.smoke && options.publish {
        return Err(CliError::Usage("a smoke run cannot be published".into()));
    }
    if options.smoke && options.resume.is_some() {
        return Err(CliError::Usage("smoke runs are not resumable".into()));
    }
    Ok(options)
}

fn preflight(options: &Options) -> Result<(), CliError> {
    for (tool, setup) in [
        ("cargo", rust_setup()),
        ("rustc", rust_setup()),
        ("uv", uv_setup()),
    ] {
        if !command_exists(tool) {
            return Err(CliError::MissingTool { tool, setup });
        }
    }
    #[cfg(windows)]
    let compiler = "cl";
    #[cfg(not(windows))]
    let compiler = "cc";
    if !command_exists(compiler) {
        return Err(CliError::MissingTool {
            tool: compiler,
            setup: compiler_setup(),
        });
    }
    println!("  prerequisites cargo, rustc, uv, and {compiler} are available");

    #[cfg(target_os = "macos")]
    if options.energy {
        checked(
            Command::new("sudo").arg("-v"),
            "authorize macOS energy sampling",
        )?;
    }
    #[cfg(windows)]
    if options.energy {
        return Err(CliError::Evidence(
            "energy measurement is not available on Windows; run without --energy".into(),
        ));
    }
    Ok(())
}

fn build_harness(root: &Path) -> Result<(), CliError> {
    println!("\n[1/4] Building release participants and evidence tools");
    let rustflags = benchmark_rustflags();
    println!("  rustflags    {rustflags}");
    checked(
        Command::new("cargo")
            .env("RUSTFLAGS", rustflags)
            .arg("build")
            .arg("--quiet")
            .arg("--release")
            .arg("--manifest-path")
            .arg(root.join("Cargo.toml"))
            .args([
                "--bin",
                "participant_node",
                "--bin",
                "benchmark_runner",
                "--bin",
                "render_results",
                "--bin",
                "describe_host",
            ]),
        "build benchmark harness",
    )
}

fn benchmark_rustflags() -> String {
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str("-C target-cpu=native");
    #[cfg(target_arch = "aarch64")]
    flags.push_str(" --cfg aes_armv8");
    flags
}

fn prepare_reference(root: &Path) -> Result<(), CliError> {
    println!("[2/4] Preparing locked compiled RNS 1.4.0 reference");
    let reference = root.join("reference");
    let environment = reference.join(".venv");
    let cache = reference.join(".object-cache/uv");
    std::fs::create_dir_all(&cache).map_err(|source| CliError::Io {
        action: "create uv cache",
        path: cache.clone(),
        source,
    })?;
    checked(
        Command::new("uv")
            .env("UV_CACHE_DIR", &cache)
            .args(["venv", "--python", "3.13", "--allow-existing"])
            .arg(&environment),
        "create managed Python 3.13 environment",
    )?;
    checked(
        Command::new("uv")
            .env("UV_CACHE_DIR", &cache)
            .arg("pip")
            .arg("sync")
            .arg("--python")
            .arg(reference_python(&reference))
            .arg(reference.join("requirements.lock")),
        "sync compiled-reference dependencies",
    )?;
    checked(
        Command::new(reference_python(&reference))
            .arg(reference.join("compiled_reference.py"))
            .arg("--verify-only"),
        "verify compiled RNS 1.4.0",
    )?;
    checked(
        Command::new(reference_python(&reference))
            .arg(reference.join("workload_vectors.py"))
            .arg(root.join("scenarios/workload-vectors.json")),
        "verify deterministic Rust/Python workload vectors",
    )
}

fn describe_host(root: &Path, output: &Path) -> Result<(), CliError> {
    println!("[3/4] Recording this machine and measurement capabilities");
    checked(
        Command::new(binary(root, "describe_host"))
            .env("BENCHMARK_RESULTS_DIR", output)
            .env("BENCHMARK_CANONICAL_RESULTS_DIR", root.join("results")),
        "describe benchmark host",
    )
}

fn render(root: &Path, results: &Path) -> Result<(), CliError> {
    println!("[4/4] Rendering complete result tables");
    let render_dir = if results == root.join("results") {
        root.to_path_buf()
    } else {
        results.to_path_buf()
    };
    checked(
        Command::new(binary(root, "render_results"))
            .env("BENCHMARK_RESULTS_DIR", results)
            .env("BENCHMARK_RENDER_DIR", render_dir),
        "render benchmark results",
    )
}

fn read_suite(run_dir: &Path) -> Result<CompletedSuite, CliError> {
    let path = run_dir.join("suite.json");
    let body = std::fs::read_to_string(&path).map_err(|source| CliError::Io {
        action: "read suite evidence",
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&body)
        .map_err(|error| CliError::Evidence(format!("parse {}: {error}", path.display())))
}

fn validate_completed_suite(suite: &CompletedSuite, options: &Options) -> Result<(), CliError> {
    if suite.schema != 1 || !suite.complete || !suite.failures.is_empty() {
        return Err(CliError::Evidence(format!(
            "suite {} is incomplete: {:?}",
            suite.suite_id, suite.failures
        )));
    }
    if !is_full_sha(&suite.source_commit) {
        return Err(CliError::Evidence(
            "suite source identity is not a full 40-character Git SHA".into(),
        ));
    }
    if suite.source_fingerprint.len() != 64
        || !suite
            .source_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CliError::Evidence(
            "suite source fingerprint is not a 64-character SHA-256 digest".into(),
        ));
    }
    if !options.smoke
        && (suite.samples_per_cell != 3
            || suite.duration_ms != 30_000
            || suite.selected_cells != 20
            || suite.matrix_cells != 20
            || !suite.reference_verified)
    {
        return Err(CliError::Evidence(
            "release suite lacks the complete 20-cell matrix or compiled-reference proof".into(),
        ));
    }
    if options.publish && suite.source_dirty {
        return Err(CliError::Evidence(
            "publication refuses evidence measured from a dirty source tree".into(),
        ));
    }
    if options.energy && !suite.energy_available {
        return Err(CliError::Evidence(
            "--energy was requested, but the completed suite contains no energy measurements"
                .into(),
        ));
    }
    Ok(())
}

fn validate_resume(run_dir: &Path, suite_id: &str, source: &SourceState) -> Result<(), CliError> {
    if !run_dir.join("suite.json").exists() {
        return Err(CliError::Evidence(format!(
            "cannot resume {suite_id}: no local suite exists at {}",
            run_dir.display()
        )));
    }
    let suite = read_suite(run_dir)?;
    if !resume_compatible(&suite, suite_id, &source.commit, &source.fingerprint) {
        return Err(CliError::Evidence(format!(
            "suite {suite_id} is incompatible with this exact source state or release profile"
        )));
    }
    println!("  resume      compatible suite found; completed conformant samples will be retained");
    Ok(())
}

fn resume_compatible(
    suite: &CompletedSuite,
    suite_id: &str,
    current_commit: &str,
    current_fingerprint: &str,
) -> bool {
    suite.suite_id == suite_id
        && suite.schema == 1
        && suite.samples_per_cell == 3
        && suite.duration_ms == 30_000
        && suite.matrix_cells == 20
        && suite.source_commit == current_commit
        && suite.source_fingerprint == current_fingerprint
}

fn promote(root: &Path, run_dir: &Path, suite: &CompletedSuite) -> Result<(), CliError> {
    let host = suite
        .host
        .as_deref()
        .ok_or_else(|| CliError::Evidence("complete suite has no host".into()))?;
    let host_root = root.join("results").join(host);
    let suite_root = host_root.join("suites").join(&suite.suite_id);
    if suite_root.exists() {
        return Err(CliError::Evidence(format!(
            "immutable suite destination already exists: {}",
            suite_root.display()
        )));
    }
    let staging = root
        .join(".benchmark-runs")
        .join(format!(".publish-{}", suite.suite_id));
    if staging.exists() {
        return Err(CliError::Evidence(format!(
            "staged publication already exists: {}",
            staging.display()
        )));
    }
    copy_tree(&run_dir.join(host), &staging)?;
    copy_file(&run_dir.join("suite.json"), &staging.join("suite.json"))?;
    std::fs::create_dir_all(suite_root.parent().expect("suite destination has a parent")).map_err(
        |source| CliError::Io {
            action: "create immutable suite parent",
            path: suite_root.clone(),
            source,
        },
    )?;
    std::fs::rename(&staging, &suite_root).map_err(|source| CliError::Io {
        action: "install immutable suite",
        path: suite_root.clone(),
        source,
    })?;

    let host_temporary = host_root.join(format!(".host-{}.tmp", suite.suite_id));
    copy_file(&run_dir.join(host).join("host.json"), &host_temporary)?;

    let current = CurrentSuite {
        schema: 1,
        suite_id: &suite.suite_id,
        source_commit: &suite.source_commit,
        path: format!("suites/{}", suite.suite_id),
    };
    let current_path = host_root.join("current.json");
    let temporary = host_root.join(format!(".current-{}.tmp", suite.suite_id));
    std::fs::write(
        &temporary,
        serde_json::to_string_pretty(&current).expect("serialize current suite") + "\n",
    )
    .map_err(|source| CliError::Io {
        action: "write staged current-suite pointer",
        path: temporary.clone(),
        source,
    })?;
    for scenario in
        benchmarks::load_catalog().map_err(|error| CliError::Evidence(error.to_string()))?
    {
        let legacy = host_root.join(scenario.name.as_str());
        if legacy.exists() {
            std::fs::remove_dir_all(&legacy).map_err(|source| CliError::Io {
                action: "remove superseded flat result directory",
                path: legacy,
                source,
            })?;
        }
    }
    replace_file(&host_temporary, &host_root.join("host.json")).map_err(|source| CliError::Io {
        action: "install host descriptor",
        path: host_root.join("host.json"),
        source,
    })?;
    replace_file(&temporary, &current_path).map_err(|source| CliError::Io {
        action: "install current-suite pointer",
        path: current_path,
        source,
    })?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt as _;

    if !destination.exists() {
        return std::fs::rename(temporary, destination);
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
    let replaced = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let success = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            1,
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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), CliError> {
    std::fs::create_dir_all(destination).map_err(|source_error| CliError::Io {
        action: "create suite directory",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    for entry in std::fs::read_dir(source).map_err(|source_error| CliError::Io {
        action: "read local suite directory",
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| CliError::Io {
            action: "read local suite entry",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), CliError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source_error| CliError::Io {
            action: "create evidence parent",
            path: parent.to_path_buf(),
            source: source_error,
        })?;
    }
    std::fs::copy(source, destination).map_err(|source_error| CliError::Io {
        action: "copy evidence",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    Ok(())
}

fn require_clean_source() -> Result<(), CliError> {
    if !source_is_clean()? {
        return Err(CliError::Evidence(
            "publication requires a clean tracked worktree; commit the harness first".into(),
        ));
    }
    Ok(())
}

fn source_is_clean() -> Result<bool, CliError> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .map_err(|source| CliError::Io {
            action: "inspect source state",
            path: PathBuf::from(".git"),
            source,
        })?;
    if !output.status.success() {
        return Err(CliError::Command {
            purpose: "inspect source state",
            status: output.status,
        });
    }
    Ok(output.stdout.is_empty())
}

fn source_state() -> Result<SourceState, CliError> {
    let commit = git_output(&["rev-parse", "HEAD"])?;
    let repository = PathBuf::from(git_output(&["rev-parse", "--show-toplevel"])?);
    let mut hash = Sha256::new();
    hash.update(b"tracked-diff\0");
    hash.update(git_bytes_in(
        &repository,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--"],
    )?);
    hash.update(b"untracked-files\0");
    let untracked = git_output_in(&repository, &["ls-files", "--others", "--exclude-standard"])?;
    for relative in untracked.lines().filter(|line| !line.is_empty()) {
        hash.update(relative.as_bytes());
        hash.update([0]);
        let path = repository.join(relative);
        let bytes = std::fs::read(&path).map_err(|source| CliError::Io {
            action: "fingerprint untracked source",
            path,
            source,
        })?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(SourceState {
        commit,
        fingerprint: format!("{:x}", hash.finalize()),
    })
}

fn git_bytes_in(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, CliError> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(arguments)
        .output()
        .map_err(|source| CliError::Io {
            action: "fingerprint source state",
            path: PathBuf::from(".git"),
            source,
        })?;
    if !output.status.success() {
        return Err(CliError::Command {
            purpose: "fingerprint source state",
            status: output.status,
        });
    }
    Ok(output.stdout)
}

fn git_output_in(repository: &Path, arguments: &[&str]) -> Result<String, CliError> {
    Ok(
        String::from_utf8_lossy(&git_bytes_in(repository, arguments)?)
            .trim()
            .to_string(),
    )
}

fn git_output(arguments: &[&str]) -> Result<String, CliError> {
    let output = Command::new("git")
        .args(arguments)
        .output()
        .map_err(|source| CliError::Io {
            action: "inspect source identity",
            path: PathBuf::from(".git"),
            source,
        })?;
    if !output.status.success() {
        return Err(CliError::Command {
            purpose: "inspect source identity",
            status: output.status,
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn command_exists(program: &str) -> bool {
    #[cfg(windows)]
    if program == "cl" {
        return Command::new("where")
            .arg("cl.exe")
            .output()
            .is_ok_and(|output| output.status.success())
            || vswhere_finds_msvc();
    }
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(windows)]
fn vswhere_finds_msvc() -> bool {
    let program_files_x86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let vswhere = Path::new(&program_files_x86)
        .join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        })
}

#[cfg(target_os = "macos")]
fn rust_setup() -> &'static str {
    "run `brew install rustup && rustup-init`"
}
#[cfg(target_os = "linux")]
fn rust_setup() -> &'static str {
    "run `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`"
}
#[cfg(windows)]
fn rust_setup() -> &'static str {
    "run `winget install Rustlang.Rustup`"
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn rust_setup() -> &'static str {
    "install Rust through rustup"
}

#[cfg(target_os = "macos")]
fn uv_setup() -> &'static str {
    "run `brew install uv`"
}
#[cfg(target_os = "linux")]
fn uv_setup() -> &'static str {
    "run `curl -LsSf https://astral.sh/uv/install.sh | sh`"
}
#[cfg(windows)]
fn uv_setup() -> &'static str {
    "run `winget install --id=astral-sh.uv -e`"
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn uv_setup() -> &'static str {
    "install uv from https://docs.astral.sh/uv/"
}

#[cfg(target_os = "macos")]
fn compiler_setup() -> &'static str {
    "run `xcode-select --install`"
}
#[cfg(target_os = "linux")]
fn compiler_setup() -> &'static str {
    "run `sudo apt-get install build-essential` (or your distribution's C compiler package)"
}
#[cfg(windows)]
fn compiler_setup() -> &'static str {
    "install Visual Studio Build Tools with the Desktop development with C++ workload"
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn compiler_setup() -> &'static str {
    "install the platform C compiler required by Cython"
}

fn checked(command: &mut Command, purpose: &'static str) -> Result<(), CliError> {
    let status = command.status().map_err(|source| CliError::Io {
        action: "start command",
        path: PathBuf::from(command.get_program()),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Command { purpose, status })
    }
}

fn binary(root: &Path, name: &str) -> PathBuf {
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    };
    root.join("target/release").join(executable)
}

fn reference_python(reference: &Path) -> PathBuf {
    if cfg!(windows) {
        reference.join(".venv/Scripts/python.exe")
    } else {
        reference.join(".venv/bin/python")
    }
}

fn print_energy_follow_up() {
    println!("{}", energy_follow_up());
}

fn energy_follow_up() -> &'static str {
    #[cfg(target_os = "macos")]
    return "ENERGY optional: re-run as your normal user with `cargo benchmark --energy`; only powermetrics receives sudo privilege.";
    #[cfg(target_os = "linux")]
    return "ENERGY optional: make the detected RAPL energy_uj files readable, then run `cargo benchmark --energy`.";
    #[cfg(windows)]
    return "ENERGY unavailable on Windows; throughput, RTT, conformance, and role memory are complete.";
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    return "ENERGY unavailable on this platform; it does not affect benchmark conformance.";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_suite() -> CompletedSuite {
        CompletedSuite {
            schema: 1,
            suite_id: uuid::Uuid::new_v4().to_string(),
            source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            source_fingerprint: "a".repeat(64),
            source_dirty: false,
            samples_per_cell: 3,
            duration_ms: 30_000,
            selected_cells: 20,
            matrix_cells: 20,
            complete: true,
            host: Some("test-host".into()),
            energy_available: false,
            reference_verified: true,
            failures: Vec::new(),
        }
    }

    fn temp_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("prns-benchmark-{label}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn public_options_are_unambiguous() {
        let parsed = parse_options(
            ["--publish", "--energy", "--resume", "suite-1"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("valid options");
        assert!(parsed.publish);
        assert!(parsed.energy);
        assert_eq!(parsed.resume.as_deref(), Some("suite-1"));
    }

    #[test]
    fn smoke_cannot_publish() {
        let error = parse_options(["--smoke", "--publish"].into_iter().map(str::to_string))
            .err()
            .expect("invalid combination");
        assert!(error.to_string().contains("cannot be published"));
    }

    #[test]
    fn release_and_publication_guards_reject_incomplete_or_dirty_evidence() {
        let mut suite = completed_suite();
        validate_completed_suite(&suite, &Options::default()).expect("complete release suite");
        suite.samples_per_cell = 2;
        assert!(validate_completed_suite(&suite, &Options::default()).is_err());
        suite.samples_per_cell = 3;
        suite.source_dirty = true;
        assert!(validate_completed_suite(
            &suite,
            &Options {
                publish: true,
                ..Options::default()
            }
        )
        .is_err());
    }

    #[test]
    fn energy_notice_explains_the_platform_without_affecting_conformance() {
        let notice = energy_follow_up();
        assert!(notice.starts_with("ENERGY"));
        #[cfg(windows)]
        assert!(notice.contains("throughput, RTT, conformance, and role memory"));
        #[cfg(target_os = "macos")]
        assert!(notice.contains("only powermetrics receives sudo"));
        #[cfg(target_os = "linux")]
        assert!(notice.contains("RAPL energy_uj"));
    }

    #[test]
    fn resume_is_bound_to_the_same_uuid_profile_and_exact_sha() {
        let root = temp_test_dir("resume");
        std::fs::create_dir_all(&root).expect("temporary run directory");
        let mut suite = completed_suite();
        suite.complete = false;
        suite.suite_id = uuid::Uuid::new_v4().to_string();
        suite.source_commit = git_output(&["rev-parse", "HEAD"]).expect("source SHA");
        std::fs::write(
            root.join("suite.json"),
            serde_json::to_string(&suite).expect("suite JSON"),
        )
        .expect("write suite");
        assert!(resume_compatible(
            &suite,
            &suite.suite_id,
            &suite.source_commit,
            &suite.source_fingerprint
        ));
        let different_fingerprint = "b".repeat(64);
        assert!(!resume_compatible(
            &suite,
            &suite.suite_id,
            &suite.source_commit,
            &different_fingerprint
        ));
        suite.source_dirty = true;
        assert!(resume_compatible(
            &suite,
            &suite.suite_id,
            &suite.source_commit,
            &suite.source_fingerprint
        ));
        suite.source_dirty = false;
        suite.duration_ms = 29_999;
        std::fs::write(
            root.join("suite.json"),
            serde_json::to_string(&suite).expect("suite JSON"),
        )
        .expect("write suite");
        assert!(!resume_compatible(
            &suite,
            &suite.suite_id,
            &suite.source_commit,
            &suite.source_fingerprint
        ));
        std::fs::remove_dir_all(root).expect("remove owned temporary run directory");
    }

    #[test]
    fn promotion_installs_an_immutable_suite_then_the_current_pointer() {
        let root = temp_test_dir("promotion");
        let run = root.join("local");
        let suite = completed_suite();
        let host = suite.host.as_deref().expect("host");
        std::fs::create_dir_all(run.join(host).join("request-response"))
            .expect("local evidence tree");
        std::fs::write(run.join(host).join("host.json"), "{}\n").expect("host evidence");
        std::fs::write(run.join("suite.json"), "{}\n").expect("suite evidence");
        std::fs::write(
            run.join(host)
                .join("request-response")
                .join("personal-rns--personal-rns.jsonl"),
            "{}\n",
        )
        .expect("result evidence");

        promote(&root, &run, &suite).expect("complete suite promotes");
        let suite_root = root
            .join("results")
            .join(host)
            .join("suites")
            .join(&suite.suite_id);
        assert!(suite_root.join("suite.json").is_file());
        let current: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("results").join(host).join("current.json"))
                .expect("current pointer"),
        )
        .expect("current JSON");
        assert_eq!(current["suite_id"], suite.suite_id);
        assert!(promote(&root, &run, &suite).is_err(), "suite is immutable");
        std::fs::remove_dir_all(root).expect("remove owned temporary publication tree");
    }
}
