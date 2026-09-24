use personal_hopspot_appliance::{BootTime, RadioPlan};
use serde::Serialize;

use super::*;

#[derive(Serialize)]
struct Inspection<'a> {
    schema: u8,
    board: &'a Board,
    boot: BootTime,
    kernel: String,
    signer_public_key_sha256: String,
    resources: Resources,
    radio_plan: RadioPlan,
}

#[derive(Serialize)]
struct Resources {
    overlay_available_bytes: u64,
    ram_available_bytes: u64,
    flash_reserve_bytes: u64,
    ram_reserve_bytes: u64,
    max_compressed_bytes: u64,
    max_executable_bytes: u64,
}

pub(super) fn print(
    options: &Options,
    board: &Board,
    profile: &Path,
    public_key: &str,
) -> Result<(), CommandError> {
    let radio_plan = radio::inspect_profile(profile, board)?;
    let overlay = filesystem_space(Path::new("/overlay"), options.flash_reserve_bytes)?;
    let ram = ram_space(options.ram_reserve_bytes)?;
    let snapshot = Inspection {
        schema: 1,
        board,
        boot: radio::boot_time(&options.root, board)?,
        kernel: fs::read_to_string("/proc/sys/kernel/osrelease")?
            .trim()
            .to_owned(),
        signer_public_key_sha256: prns_flash_manifest::sha256_hex(public_key.as_bytes()),
        resources: Resources {
            overlay_available_bytes: overlay.available_bytes,
            ram_available_bytes: ram.available_bytes,
            flash_reserve_bytes: overlay.reserve_bytes,
            ram_reserve_bytes: ram.reserve_bytes,
            max_compressed_bytes: options.max_compressed_bytes,
            max_executable_bytes: options.max_executable_bytes,
        },
        radio_plan,
    };
    println!("{}", serde_json::to_string(&snapshot)?);
    Ok(())
}

pub(super) fn ram_space(reserve_bytes: u64) -> Result<SpaceBudget, CommandError> {
    let mut space = filesystem_space(Path::new("/tmp"), reserve_bytes)?;
    space.available_bytes =
        space
            .available_bytes
            .min(memory_available_bytes(&fs::read_to_string(
                "/proc/meminfo",
            )?)?);
    Ok(space)
}

fn memory_available_bytes(memory: &str) -> Result<u64, CommandError> {
    let line = memory
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .ok_or(CommandError::Capacity)?;
    let mut fields = line.split_whitespace();
    let kib = fields
        .next()
        .ok_or(CommandError::Capacity)?
        .parse::<u64>()
        .map_err(|_| CommandError::Capacity)?;
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err(CommandError::Capacity);
    }
    kib.checked_mul(1024).ok_or(CommandError::Capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_availability_requires_a_real_bounded_measurement() {
        assert_eq!(
            memory_available_bytes("MemFree: 20 kB\nMemAvailable: 1024 kB\n").unwrap(),
            1048576
        );
        for unavailable in [
            "MemFree: 20 kB\n",
            "MemAvailable: 1024 MB\n",
            "MemAvailable: 1024\n",
            "MemAvailable: 18446744073709551615 kB\n",
        ] {
            assert!(matches!(
                memory_available_bytes(unavailable),
                Err(CommandError::Capacity)
            ));
        }
    }
}
