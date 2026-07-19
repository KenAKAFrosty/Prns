use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::boards::{
    BoardBackend, BoardTarget, EspImageSpec, ESP32S3_TARGET, T_ECHO_BASE, T_ECHO_FAMILY,
    T_ECHO_PROFILE,
};
use crate::toolchain::{capture_stdout, configure_esp_toolchain, run_status, rust_host_triple};
use crate::wifi::{hopspot_config_image_bytes, HOPSPOT_CONFIG_OFFSET};
use crate::{ui, AppResult};

pub(crate) struct BuildOutput {
    pub(crate) artifact: PathBuf,
    metadata: PathBuf,
    web_manifest: Option<PathBuf>,
    profile: &'static str,
    sha256: String,
    size: u64,
}

pub(crate) struct EspFirmware {
    pub(crate) elf: PathBuf,
    pub(crate) partition_table: PathBuf,
}

pub(crate) fn build_board(
    board: &BoardTarget,
    repo: &Path,
    out_root: &Path,
) -> AppResult<BuildOutput> {
    ensure_supported(board)?;
    match board.backend {
        BoardBackend::TEchoUf2 => build_t_echo(repo, out_root),
        BoardBackend::EspFlash(spec) => build_esp_board(board, spec, repo, out_root),
    }
}

pub(crate) fn ensure_supported(board: &BoardTarget) -> AppResult<()> {
    let _ = board;
    Ok(())
}

pub(crate) fn build_t_echo(repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    ui::print_section("Building LilyGO T-Echo");
    let crate_dir = repo.join("personal-hopspot").join("t-echo");
    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--no-default-features")
        .arg("--features")
        .arg(T_ECHO_PROFILE)
        .current_dir(&crate_dir);
    run_status(
        &mut cargo,
        "cargo build --release --no-default-features --features hopspot-t-echo",
    )?;

    let host_triple = rust_host_triple()?;
    let sysroot = capture_stdout(Command::new("rustc").arg("--print").arg("sysroot"), "rustc")?;
    let objcopy = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host_triple.trim())
        .join("bin")
        .join("llvm-objcopy");

    let work_dir = repo
        .join("target")
        .join("flash-artifacts")
        .join("work")
        .join("t-echo");
    let board_out = out_root
        .join("firmware")
        .join("hopspot")
        .join("t-echo")
        .join("latest");
    fs::create_dir_all(&work_dir)
        .map_err(|err| format!("failed to create {}: {err}", work_dir.display()))?;
    fs::create_dir_all(&board_out)
        .map_err(|err| format!("failed to create {}: {err}", board_out.display()))?;

    let elf = crate_dir
        .join("target")
        .join("thumbv7em-none-eabihf")
        .join("release")
        .join("t-echo");
    let bin = work_dir.join("t-echo.bin");
    let uf2 = board_out.join("t-echo.uf2");
    let metadata = board_out.join("t-echo.uf2.json");

    run_status(
        Command::new(&objcopy)
            .arg("-O")
            .arg("binary")
            .arg(&elf)
            .arg(&bin),
        "llvm-objcopy",
    )?;
    run_status(
        Command::new("python3")
            .arg(repo.join("scripts").join("bin2uf2.py"))
            .arg(&bin)
            .arg(&uf2)
            .arg(T_ECHO_BASE)
            .arg(T_ECHO_FAMILY),
        "bin2uf2.py",
    )?;

    let sha256 = sha256_file(&uf2)?;
    let size = fs::metadata(&uf2)
        .map_err(|err| format!("failed to inspect {}: {err}", uf2.display()))?
        .len();
    write_metadata(&metadata, &sha256, size, T_ECHO_PROFILE)?;

    Ok(BuildOutput {
        artifact: uf2,
        metadata,
        web_manifest: None,
        profile: T_ECHO_PROFILE,
        sha256,
        size,
    })
}

fn build_esp_board(
    board: &BoardTarget,
    spec: &EspImageSpec,
    repo: &Path,
    out_root: &Path,
) -> AppResult<BuildOutput> {
    let firmware = build_esp_firmware(board, spec, repo)?;

    let board_out = out_root
        .join("firmware")
        .join("hopspot")
        .join(board.slug)
        .join("latest");
    fs::create_dir_all(&board_out)
        .map_err(|err| format!("failed to create {}: {err}", board_out.display()))?;

    let artifact = board_out.join(spec.artifact);
    let metadata = board_out.join(format!("{}.json", spec.artifact));
    let web_manifest = board_out.join("manifest.json");
    if spec.wifi_configurable {
        fs::write(
            board_out.join("hopspot-config-empty.bin"),
            hopspot_config_image_bytes(None),
        )
        .map_err(|err| {
            format!(
                "failed to write {}: {err}",
                board_out.join("hopspot-config-empty.bin").display()
            )
        })?;
    }

    run_status(
        Command::new("espflash")
            .arg("save-image")
            .arg("--chip")
            .arg(spec.chip)
            .arg("--flash-size")
            .arg(spec.flash_size)
            .arg("--partition-table")
            .arg(&firmware.partition_table)
            .arg("--target-app-partition")
            .arg("factory")
            .arg("--merge")
            .arg("--skip-padding")
            .arg(&firmware.elf)
            .arg(&artifact),
        "espflash save-image",
    )?;

    let sha256 = sha256_file(&artifact)?;
    let size = fs::metadata(&artifact)
        .map_err(|err| format!("failed to inspect {}: {err}", artifact.display()))?
        .len();
    write_esp_metadata(&metadata, board.slug, spec, &sha256, size)?;
    write_esp_web_manifest(&web_manifest, spec)?;

    Ok(BuildOutput {
        artifact,
        metadata,
        web_manifest: Some(web_manifest),
        profile: spec.profile,
        sha256,
        size,
    })
}

pub(crate) fn build_esp_firmware(
    board: &BoardTarget,
    spec: &EspImageSpec,
    repo: &Path,
) -> AppResult<EspFirmware> {
    prepare_embedded_site_bundle(spec, repo)?;

    let section = format!("Building {}", board.name);
    ui::print_section(&section);
    let crate_dir = repo.join("personal-hopspot").join("embedded").join("esp32");
    let elf = crate_dir
        .join("target")
        .join(spec.target)
        .join("release")
        .join("personal-hopspot-esp32");
    let partition_table = crate_dir.join(spec.partition_table);

    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("personal-hopspot-esp32")
        .arg("--target")
        .arg(spec.target)
        .arg("-Zbuild-std=core,alloc");
    if spec.no_default_features {
        cargo.arg("--no-default-features");
    }
    cargo
        .arg("--features")
        .arg(spec.profile)
        .current_dir(&crate_dir);
    if spec.target == ESP32S3_TARGET {
        let linker = configure_esp_toolchain(&mut cargo)?;
        ui::print_key_value("xtensa gcc", &linker.display().to_string());
    }
    let build_label = format!(
        "cargo build --release --bin personal-hopspot-esp32 --target {} -Zbuild-std=core,alloc {}--features {}",
        spec.target,
        if spec.no_default_features {
            "--no-default-features "
        } else {
            ""
        },
        spec.profile
    );
    run_status(&mut cargo, &build_label)?;

    Ok(EspFirmware {
        elf,
        partition_table,
    })
}

fn prepare_embedded_site_bundle(spec: &EspImageSpec, repo: &Path) -> AppResult<()> {
    if spec.target != ESP32S3_TARGET {
        return Ok(());
    }

    let site_dir = repo.join("docs").join("website");
    let output_dir = site_dir
        .join("target")
        .join("dx")
        .join("reticulum-site")
        .join("release")
        .join("web")
        .join("public");

    ui::print_section("Building embedded docs bundle");
    ui::print_key_value("mode", "Hopspot SoftAP");
    ui::print_key_value("output", &output_dir.display().to_string());

    let mut dx = Command::new("dx");
    dx.env("PRNS_EMBEDDED_SITE", "1")
        .env_remove("PRNS_FLASH_ARTIFACT_ROOT")
        .arg("build")
        .arg("--platform")
        .arg("web")
        .arg("--debug-symbols")
        .arg("false")
        .arg("--release")
        .current_dir(&site_dir);
    run_status(
        &mut dx,
        "dx build --platform web --debug-symbols false --release",
    )?;

    if !output_dir.join("index.html").is_file() {
        return Err(format!(
            "embedded docs bundle did not produce {}",
            output_dir.join("index.html").display()
        ));
    }
    if !output_dir.join("source.zip").is_file() {
        return Err(format!(
            "embedded docs bundle did not include {}",
            output_dir.join("source.zip").display()
        ));
    }

    Ok(())
}

pub(crate) fn print_build_output(output: &BuildOutput) {
    println!();
    ui::print_section("Artifact ready");
    ui::print_key_value("artifact", &output.artifact.display().to_string());
    ui::print_key_value("metadata", &output.metadata.display().to_string());
    if let Some(web_manifest) = &output.web_manifest {
        ui::print_key_value("web manifest", &web_manifest.display().to_string());
    }
    ui::print_key_value("profile", output.profile);
    ui::print_key_value("sha256", &output.sha256);
    ui::print_key_value("size", &format!("{} bytes", output.size));
}

fn write_metadata(path: &Path, sha256: &str, size: u64, profile: &str) -> AppResult<()> {
    let json = format!(
        concat!(
            "{{\n",
            "  \"board_slug\": \"t-echo\",\n",
            "  \"profile\": \"{profile}\",\n",
            "  \"format\": \"uf2\",\n",
            "  \"transport\": \"uf2-mass-storage\",\n",
            "  \"artifact\": \"t-echo.uf2\",\n",
            "  \"artifact_sha256\": \"{sha256}\",\n",
            "  \"artifact_size\": {size},\n",
            "  \"flash_base\": \"{base}\",\n",
            "  \"family\": \"{family}\",\n",
            "  \"source\": \"personal-hopspot/embedded/nrf52840\"\n",
            "}}\n"
        ),
        profile = profile,
        sha256 = sha256,
        size = size,
        base = T_ECHO_BASE,
        family = T_ECHO_FAMILY,
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn write_esp_metadata(
    path: &Path,
    board_slug: &str,
    spec: &EspImageSpec,
    sha256: &str,
    size: u64,
) -> AppResult<()> {
    let json = format!(
        concat!(
            "{{\n",
            "  \"board_slug\": \"{board_slug}\",\n",
            "  \"profile\": \"{profile}\",\n",
            "  \"format\": \"esp-bin\",\n",
            "  \"transport\": \"esp-web-serial\",\n",
            "  \"artifact\": \"{artifact}\",\n",
            "  \"artifact_sha256\": \"{sha256}\",\n",
            "  \"artifact_size\": {size},\n",
            "  \"chip\": \"{chip}\",\n",
            "  \"flash_size\": \"{flash_size}\",\n",
            "  \"partition_table\": \"personal-hopspot/embedded/esp32/{partition_table}\",\n",
            "  \"config_offset\": {config_offset},\n",
            "  \"config_artifact\": {config_artifact},\n",
            "  \"source\": \"personal-hopspot/embedded/esp32\"\n",
            "}}\n"
        ),
        board_slug = board_slug,
        profile = spec.profile,
        artifact = spec.artifact,
        sha256 = sha256,
        size = size,
        chip = spec.chip,
        flash_size = spec.flash_size,
        partition_table = spec.partition_table,
        config_offset = if spec.wifi_configurable {
            format!("\"0x{HOPSPOT_CONFIG_OFFSET:x}\"")
        } else {
            "null".to_string()
        },
        config_artifact = if spec.wifi_configurable {
            "\"hopspot-config-empty.bin\"".to_string()
        } else {
            "null".to_string()
        },
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn write_esp_web_manifest(path: &Path, spec: &EspImageSpec) -> AppResult<()> {
    let config_part = if spec.wifi_configurable {
        format!(
            ",\n        {{ \"path\": \"hopspot-config-empty.bin\", \"offset\": {} }}",
            HOPSPOT_CONFIG_OFFSET
        )
    } else {
        String::new()
    };
    let json = format!(
        concat!(
            "{{\n",
            "  \"name\": \"{name}\",\n",
            "  \"version\": \"preview\",\n",
            "  \"new_install_prompt_erase\": true,\n",
            "  \"new_install_improv_wait_time\": 0,\n",
            "  \"builds\": [\n",
            "    {{\n",
            "      \"chipFamily\": \"{chip_family}\",\n",
            "      \"improv\": false,\n",
            "      \"parts\": [\n",
            "        {{ \"path\": \"{artifact}\", \"offset\": 0 }}{config_part}\n",
            "      ]\n",
            "    }}\n",
            "  ]\n",
            "}}\n"
        ),
        name = spec.web_name,
        chip_family = spec.chip_family,
        artifact = spec.artifact,
        config_part = config_part,
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn sha256_file(path: &Path) -> AppResult<String> {
    if let Ok(hash) = capture_stdout(
        Command::new("shasum").arg("-a").arg("256").arg(path),
        "shasum",
    ) {
        return first_word(&hash).ok_or_else(|| "shasum produced no hash".to_string());
    }
    if let Ok(hash) = capture_stdout(Command::new("sha256sum").arg(path), "sha256sum") {
        return first_word(&hash).ok_or_else(|| "sha256sum produced no hash".to_string());
    }
    Err("missing shasum or sha256sum".to_string())
}

fn first_word(value: &str) -> Option<String> {
    value.split_whitespace().next().map(str::to_string)
}

pub(crate) fn default_artifact_root(repo: &Path) -> PathBuf {
    repo.join("target").join("flash-artifacts")
}
