use super::*;

#[test]
fn canonical_matrix_has_fourteen_unique_profile_bound_targets(
) -> Result<(), Box<dyn std::error::Error>> {
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let targets = matrix
        .iter()
        .map(|target| {
            (
                target.id(),
                target.profile().id.0,
                target.adapter().rust_target(),
                target.adapter().id().as_str(),
                target.platform(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        targets,
        [
            (
                "heltec-v4",
                "heltec-v4",
                "xtensa-esp32s3-none-elf",
                "xtensa-esp32s3-gnu-ld",
                TargetPlatform::Esp
            ),
            (
                "heltec-v4-r8",
                "heltec-v4-r8",
                "xtensa-esp32s3-none-elf",
                "xtensa-esp32s3-gnu-ld",
                TargetPlatform::Esp
            ),
            (
                "heltec-e290",
                "heltec-e290",
                "xtensa-esp32s3-none-elf",
                "xtensa-esp32s3-gnu-ld",
                TargetPlatform::Esp
            ),
            (
                "heltec-wireless-stick-lite-v3",
                "heltec-wireless-stick-lite-v3",
                "xtensa-esp32s3-none-elf",
                "xtensa-esp32s3-gnu-ld",
                TargetPlatform::Esp
            ),
            (
                "t-beam-supreme",
                "t-beam-supreme",
                "xtensa-esp32s3-none-elf",
                "xtensa-esp32s3-gnu-ld",
                TargetPlatform::Esp
            ),
            (
                "xiao-esp32-c6",
                "xiao-esp32-c6",
                "riscv32imac-unknown-none-elf",
                "riscv32imac-rust-lld",
                TargetPlatform::Esp
            ),
            (
                "t-echo-s140-v6",
                "t-echo-s140-v6",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "t-echo-s140-v7",
                "t-echo-s140-v7",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "t114",
                "t114",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "mesh-pocket-5000",
                "mesh-pocket-5000",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "mesh-pocket-10000",
                "mesh-pocket-10000",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "t096",
                "t096",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "t1000-e",
                "t1000-e",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
            (
                "mesh-tower-v2",
                "mesh-tower-v2",
                "thumbv7em-none-eabihf",
                "thumbv7em-rust-lld",
                TargetPlatform::Nrf52840
            ),
        ]
    );
    Ok(())
}

#[test]
fn unknown_targets_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    assert!(matches!(
        matrix.target("unknown"),
        Err(MatrixError::UnknownTarget(target)) if target == "unknown"
    ));
    Ok(())
}

#[test]
fn mesh_tower_alone_selects_thin_lto() -> Result<(), Box<dyn std::error::Error>> {
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let selections = matrix
        .iter()
        .map(|target| (target.id(), target.recipe_identity().configured_lto))
        .collect::<Vec<_>>();

    for (target, lto) in selections {
        let expected = if target == "mesh-tower-v2" {
            personal_hopspot_builder::LtoMode::Thin
        } else {
            personal_hopspot_builder::LtoMode::Configured
        };
        assert_eq!(lto, expected, "unexpected LTO policy for {target}");
    }
    Ok(())
}
