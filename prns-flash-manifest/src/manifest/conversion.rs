use std::fmt;

use crate::domain::TargetIdentity;
use crate::{
    AfterResetStrategy, BeforeResetStrategy, BoardBuild, BoardCatalog, BoardCatalogEntry, BoardId,
    ChipFamily, EspFlashPart, EspSerialTarget, FlashFrequency, FlashMode, ImmutableArtifactPath,
    KeyId, NrfSerialDfuArtifact, NrfSerialDfuRecovery, NrfSerialDfuTarget, PreparationProfile,
    ProvisioningDescriptor, ProvisioningFormat, ProvisioningSlot, ReleaseTarget, ReleaseVersion,
    Sha256Digest, SoftdeviceIdentity, Transport, Uf2Compatibility, Uf2MountLabel, Uf2Part,
    Uf2Target, Uf2Variant, ValidatedChannelDescriptor, ValidatedFlashManifest,
    ValidatedNrfSerialDfuCompatibility, ValidatedNrfSerialDfuSerialTransport,
    ValidatedOfflineKeySigningInfo, ValidatedReleaseInfo, CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES,
    CONFIG_SIZE, CONFIG_SSID_MAX_BYTES, CONFIG_VERSION,
};

use super::validation::{validate_target, validate_uf2_target_variant};
use super::{
    ChannelDescriptor, FlashManifest, FlashPartKind, ManifestError, ManifestTargetSetPolicy,
    ReleaseChannel, TargetManifest,
};

impl ValidatedFlashManifest {
    pub fn from_json(bytes: &[u8], catalog: &BoardCatalog) -> Result<Self, ManifestError> {
        let wire: FlashManifest = serde_json::from_slice(bytes)?;
        wire.into_validated(catalog)
    }

    pub fn from_json_with_target_set(
        bytes: &[u8],
        catalog: &BoardCatalog,
        policy: &ManifestTargetSetPolicy,
    ) -> Result<Self, ManifestError> {
        let wire: FlashManifest = serde_json::from_slice(bytes)?;
        wire.into_validated_with_target_set(catalog, policy)
    }
}

impl TargetManifest {
    pub fn into_validated(
        self,
        board: &BoardCatalogEntry,
        version: &ReleaseVersion,
    ) -> Result<ReleaseTarget, ManifestError> {
        validate_target(&self, board, version.as_str())?;
        convert_target(self, board)
    }

    pub fn into_validated_uf2_variant(
        self,
        board: &BoardCatalogEntry,
        version: &ReleaseVersion,
        softdevice: &SoftdeviceIdentity,
    ) -> Result<ReleaseTarget, ManifestError> {
        validate_uf2_target_variant(&self, board, version.as_str(), softdevice)?;
        convert_target(self, board)
    }
}

impl ValidatedChannelDescriptor {
    pub fn from_json(bytes: &[u8], expected: ReleaseChannel) -> Result<Self, ManifestError> {
        let wire: ChannelDescriptor = serde_json::from_slice(bytes)?;
        wire.validate(expected)?;
        convert_channel_descriptor(wire)
    }
}

pub(super) fn convert_manifest(
    manifest: FlashManifest,
    catalog: &BoardCatalog,
) -> Result<ValidatedFlashManifest, ManifestError> {
    let FlashManifest {
        schema_version,
        release,
        signing,
        targets,
    } = manifest;
    let version = ReleaseVersion::parse(release.version).map_err(release_domain_error)?;
    let key_id = KeyId::parse(signing.key_id).map_err(release_domain_error)?;
    let mut validated_targets = Vec::with_capacity(targets.len());
    for target in targets {
        let board = catalog.board(&target.board_slug).ok_or_else(|| {
            ManifestError::TargetSet(format!("unknown board {:?}", target.board_slug))
        })?;
        // Whole-manifest validation has already run, but keep this conversion
        // independently safe for future call sites.
        validate_target(&target, board, version.as_str())?;
        validated_targets.push(convert_target(target, board)?);
    }
    Ok(ValidatedFlashManifest {
        schema_version,
        release: ValidatedReleaseInfo {
            version,
            channel: release.channel,
            commit: release.commit,
        },
        signing: ValidatedOfflineKeySigningInfo { key_id },
        targets: validated_targets,
    })
}

pub(super) fn convert_channel_descriptor(
    descriptor: ChannelDescriptor,
) -> Result<ValidatedChannelDescriptor, ManifestError> {
    Ok(ValidatedChannelDescriptor {
        schema_version: descriptor.schema_version,
        channel: descriptor.channel,
        version: ReleaseVersion::parse(descriptor.version).map_err(release_domain_error)?,
        manifest_url: descriptor.manifest_url,
        manifest_sha256: Sha256Digest::parse(descriptor.manifest_sha256)
            .map_err(release_domain_error)?,
    })
}

fn convert_target(
    target: TargetManifest,
    board: &BoardCatalogEntry,
) -> Result<ReleaseTarget, ManifestError> {
    let TargetManifest {
        board_slug,
        display_name,
        silicon,
        interfaces,
        transport,
        expected_chip,
        flash_size,
        flash_mode,
        flash_frequency,
        before_reset,
        after_reset,
        preparation_profile,
        parts,
        variants,
        nrf_serial_dfu,
        provisioning,
        source,
    } = target;
    let identity = TargetIdentity {
        board_id: BoardId::parse(board_slug.clone())
            .map_err(|error| ManifestError::TargetSet(format!("board {board_slug:?}: {error}")))?,
        display_name,
        silicon,
        interfaces,
        preparation_profile: PreparationProfile::parse(&preparation_profile).map_err(|error| {
            ManifestError::CatalogMismatch {
                board: board_slug.clone(),
                field: error.to_string(),
            }
        })?,
        source,
    };
    match transport {
        Transport::EspSerial => {
            if !variants.is_empty() || nrf_serial_dfu.is_some() {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "ESP target contains UF2 variants".to_string(),
                });
            }
            if identity.preparation_profile != PreparationProfile::EspUsbBoot {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "ESP target requires the esp-usb-boot preparation profile".to_string(),
                });
            }
            let expected_chip = required_target_value(&board_slug, "expected_chip", expected_chip)?;
            let flash_size = required_target_value(&board_slug, "flash_size", flash_size)?;
            let flash_mode = required_target_value(&board_slug, "flash_mode", flash_mode)?;
            let flash_frequency =
                required_target_value(&board_slug, "flash_frequency", flash_frequency)?;
            let before_reset = required_target_value(&board_slug, "before_reset", before_reset)?;
            let after_reset = required_target_value(&board_slug, "after_reset", after_reset)?;
            let mut validated_parts = Vec::with_capacity(parts.len());
            for part in parts {
                if !matches!(
                    part.kind,
                    FlashPartKind::Bootloader
                        | FlashPartKind::PartitionTable
                        | FlashPartKind::Application
                ) {
                    return Err(invalid_part_values(
                        &board_slug,
                        &part.path,
                        "ESP target cannot contain a UF2 part",
                    ));
                }
                let offset = part.offset.ok_or_else(|| {
                    invalid_part_values(&board_slug, &part.path, "ESP part requires an offset")
                })?;
                validated_parts.push(EspFlashPart {
                    kind: part.kind,
                    path: ImmutableArtifactPath::parse(part.path.clone()).map_err(|error| {
                        invalid_part_values(&board_slug, &part.path, &error.to_string())
                    })?,
                    offset,
                    size: part.size,
                    sha256: Sha256Digest::parse(part.sha256).map_err(|error| {
                        invalid_part_values(&board_slug, &part.path, &error.to_string())
                    })?,
                });
            }
            let provisioning = provisioning
                .map(|slot| convert_provisioning(&board_slug, slot))
                .transpose()?;
            Ok(ReleaseTarget::EspSerial(EspSerialTarget {
                identity,
                expected_chip: ChipFamily::parse(&expected_chip).map_err(|error| {
                    ManifestError::CatalogMismatch {
                        board: board_slug.clone(),
                        field: error.to_string(),
                    }
                })?,
                flash_size,
                flash_mode: FlashMode::parse(&flash_mode).map_err(|error| {
                    ManifestError::CatalogMismatch {
                        board: board_slug.clone(),
                        field: error.to_string(),
                    }
                })?,
                flash_frequency: FlashFrequency::parse(&flash_frequency).map_err(|error| {
                    ManifestError::CatalogMismatch {
                        board: board_slug.clone(),
                        field: error.to_string(),
                    }
                })?,
                before_reset: BeforeResetStrategy::parse(&before_reset).map_err(|error| {
                    ManifestError::CatalogMismatch {
                        board: board_slug.clone(),
                        field: error.to_string(),
                    }
                })?,
                after_reset: AfterResetStrategy::parse(&after_reset).map_err(|error| {
                    ManifestError::CatalogMismatch {
                        board: board_slug.clone(),
                        field: error.to_string(),
                    }
                })?,
                parts: validated_parts,
                provisioning,
            }))
        }
        Transport::Uf2MassStorage => {
            if PreparationProfile::parse(&board.preparation_profile)
                != Ok(identity.preparation_profile)
            {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "UF2 target preparation profile disagrees with the board catalog"
                        .to_string(),
                });
            }
            let BoardBuild::Uf2(catalog_build) = &board.build else {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "UF2 target requires a cataloged UF2 build recipe".to_string(),
                });
            };
            if expected_chip.is_some()
                || flash_size.is_some()
                || flash_mode.is_some()
                || flash_frequency.is_some()
                || before_reset.is_some()
                || after_reset.is_some()
                || provisioning.is_some()
            {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "UF2 target contains ESP-only fields".to_string(),
                });
            }
            if !parts.is_empty() || nrf_serial_dfu.is_some() {
                return Err(invalid_part_values(
                    &board_slug,
                    "",
                    "UF2 target cannot contain top-level parts",
                ));
            }
            let variants = variants
                .into_iter()
                .map(|variant| {
                    let path = variant.path.clone();
                    let application_end_exclusive = catalog_build
                        .variants
                        .iter()
                        .find(|catalog_variant| {
                            catalog_variant.softdevice_family == variant.softdevice_family
                                && catalog_variant.softdevice_version == variant.softdevice_version
                                && catalog_variant.fwid == variant.fwid
                                && catalog_variant.application_base == variant.application_base
                                && catalog_variant.family_id == variant.family_id
                        })
                        .and_then(|catalog_variant| {
                            parse_hex_u32(&catalog_variant.application_end_exclusive)
                        })
                        .ok_or_else(|| {
                            invalid_part_values(
                                &board_slug,
                                &path,
                                "UF2 variant is not pinned by the board catalog",
                            )
                        })?;
                    let softdevice = SoftdeviceIdentity::parse(
                        &variant.softdevice_family,
                        variant.softdevice_version,
                    )
                    .map_err(|error| invalid_part_values(&board_slug, &path, &error.to_string()))?;
                    let fwid = parse_hex_u16(&variant.fwid).ok_or_else(|| {
                        invalid_part_values(&board_slug, &path, "FWID is not canonical hexadecimal")
                    })?;
                    let application_base =
                        parse_hex_u32(&variant.application_base).ok_or_else(|| {
                            invalid_part_values(
                                &board_slug,
                                &path,
                                "application base is not canonical hexadecimal",
                            )
                        })?;
                    let family_id = parse_hex_u32(&variant.family_id).ok_or_else(|| {
                        invalid_part_values(
                            &board_slug,
                            &path,
                            "UF2 family ID is not canonical hexadecimal",
                        )
                    })?;
                    Ok(Uf2Variant {
                        compatibility: Uf2Compatibility::new(
                            softdevice,
                            fwid,
                            application_base,
                            application_end_exclusive,
                            family_id,
                        ),
                        part: Uf2Part {
                            path: ImmutableArtifactPath::parse(variant.path.clone()).map_err(
                                |error| {
                                    invalid_part_values(
                                        &board_slug,
                                        &variant.path,
                                        &error.to_string(),
                                    )
                                },
                            )?,
                            size: variant.size,
                            sha256: Sha256Digest::parse(variant.sha256).map_err(|error| {
                                invalid_part_values(&board_slug, &variant.path, &error.to_string())
                            })?,
                        },
                    })
                })
                .collect::<Result<Vec<_>, ManifestError>>()?;
            Ok(ReleaseTarget::Uf2(Uf2Target { identity, variants }))
        }
        Transport::NrfSerialDfu => {
            let BoardBuild::NrfSerialDfu(catalog_build) = &board.build else {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "Nordic serial DFU transport requires its catalog build".to_string(),
                });
            };
            if identity.preparation_profile != PreparationProfile::T1000eNrfDfu {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field:
                        "Nordic serial DFU target requires the t1000e-nrf-dfu preparation profile"
                            .to_string(),
                });
            }
            if expected_chip.is_some()
                || flash_size.is_some()
                || flash_mode.is_some()
                || flash_frequency.is_some()
                || before_reset.is_some()
                || after_reset.is_some()
                || provisioning.is_some()
                || !parts.is_empty()
                || !variants.is_empty()
            {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "Nordic serial DFU target contains fields owned by another transport"
                        .to_string(),
                });
            }
            let manifest = nrf_serial_dfu.ok_or_else(|| ManifestError::CatalogMismatch {
                board: board_slug.clone(),
                field: "Nordic serial DFU target has no artifact contract".to_string(),
            })?;
            let serial_transport = convert_nrf_serial_dfu_serial_transport(
                &board_slug,
                &manifest.application.path,
                manifest.serial,
            )?;
            let compatibility = convert_nrf_serial_dfu_compatibility(
                &board_slug,
                &manifest.application.path,
                manifest.compatibility,
            )?;
            let application = convert_nrf_serial_dfu_artifact(
                &board_slug,
                manifest.application,
                FlashPartKind::DfuApplication,
            )?;
            let init_packet = convert_nrf_serial_dfu_artifact(
                &board_slug,
                manifest.init_packet,
                FlashPartKind::DfuInitPacket,
            )?;
            let recovery_path = manifest.recovery.artifact.path.clone();
            let recovery = NrfSerialDfuRecovery {
                mount_label: Uf2MountLabel::parse(manifest.recovery.mount_label).map_err(
                    |error| invalid_part_values(&board_slug, &recovery_path, &error.to_string()),
                )?,
                board_id_match: catalog_build.recovery.board_identity.validated().map_err(
                    |error| invalid_part_values(&board_slug, &recovery_path, &error.to_string()),
                )?,
                family_id: parse_hex_u32(&manifest.recovery.family_id).ok_or_else(|| {
                    invalid_part_values(
                        &board_slug,
                        &recovery_path,
                        "UF2 family ID is not canonical hexadecimal",
                    )
                })?,
                artifact: convert_nrf_serial_dfu_artifact(
                    &board_slug,
                    manifest.recovery.artifact,
                    FlashPartKind::Uf2,
                )?,
            };
            Ok(ReleaseTarget::NrfSerialDfu(Box::new(NrfSerialDfuTarget {
                identity,
                serial_transport,
                compatibility,
                application,
                init_packet,
                recovery,
            })))
        }
    }
}

fn convert_nrf_serial_dfu_serial_transport(
    board: &str,
    path: &str,
    serial: crate::NrfSerialDfuSerialTransport,
) -> Result<ValidatedNrfSerialDfuSerialTransport, ManifestError> {
    serial
        .into_validated()
        .map_err(|error| invalid_part_values(board, path, &error.to_string()))
}

fn convert_nrf_serial_dfu_compatibility(
    board: &str,
    path: &str,
    compatibility: crate::NrfSerialDfuCompatibility,
) -> Result<ValidatedNrfSerialDfuCompatibility, ManifestError> {
    let fwid = parse_hex_u16(&compatibility.fwid)
        .filter(|fwid| *fwid != 0xfffe)
        .ok_or_else(|| invalid_part_values(board, path, "FWID is not an exact hexadecimal ID"))?;
    let device_type = parse_hex_u16(&compatibility.device_type).ok_or_else(|| {
        invalid_part_values(board, path, "device type is not canonical hexadecimal")
    })?;
    let application_base = parse_hex_u32(&compatibility.application_base).ok_or_else(|| {
        invalid_part_values(board, path, "application base is not canonical hexadecimal")
    })?;
    let application_end_exclusive = parse_hex_u32(&compatibility.application_end_exclusive)
        .filter(|end| application_base < *end)
        .ok_or_else(|| {
            invalid_part_values(board, path, "application region is empty or malformed")
        })?;
    Ok(ValidatedNrfSerialDfuCompatibility {
        softdevice: SoftdeviceIdentity::parse(
            &compatibility.softdevice_family,
            compatibility.softdevice_version,
        )
        .map_err(|error| invalid_part_values(board, path, &error.to_string()))?,
        fwid,
        device_type,
        device_revision: compatibility.device_revision,
        application_version: compatibility.application_version,
        application_base,
        application_end_exclusive,
        bank_layout: compatibility.bank_layout,
    })
}

fn convert_nrf_serial_dfu_artifact(
    board: &str,
    part: crate::FlashPart,
    expected_kind: FlashPartKind,
) -> Result<NrfSerialDfuArtifact, ManifestError> {
    if part.kind != expected_kind || part.offset.is_some() {
        return Err(invalid_part_values(
            board,
            &part.path,
            "Nordic DFU artifact role or offset is invalid",
        ));
    }
    Ok(NrfSerialDfuArtifact {
        kind: part.kind,
        path: ImmutableArtifactPath::parse(part.path.clone())
            .map_err(|error| invalid_part_values(board, &part.path, &error.to_string()))?,
        size: part.size,
        sha256: Sha256Digest::parse(part.sha256)
            .map_err(|error| invalid_part_values(board, &part.path, &error.to_string()))?,
    })
}

fn parse_hex_u16(value: &str) -> Option<u16> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 4
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u16::from_str_radix(digits, 16).ok())
    .flatten()
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 8
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u32::from_str_radix(digits, 16).ok())
    .flatten()
}

fn required_target_value<T>(
    board: &str,
    field: &str,
    value: Option<T>,
) -> Result<T, ManifestError> {
    value.ok_or_else(|| ManifestError::CatalogMismatch {
        board: board.to_string(),
        field: format!("ESP target is missing {field}"),
    })
}

fn convert_provisioning(
    board: &str,
    slot: ProvisioningDescriptor,
) -> Result<ProvisioningSlot, ManifestError> {
    if slot.version != CONFIG_VERSION
        || slot.offset != CONFIG_OFFSET
        || slot.size != CONFIG_SIZE as u32
        || slot.ssid_max_bytes != CONFIG_SSID_MAX_BYTES
        || slot.password_max_bytes != CONFIG_PASSWORD_MAX_BYTES
    {
        return Err(ManifestError::CatalogMismatch {
            board: board.to_string(),
            field: "provisioning slot disagrees with the HSPCFG1 wire contract".to_string(),
        });
    }
    if slot.tcp_client.as_ref().is_some_and(|tcp_client| {
        tcp_client.target_format != "ipv4-or-dns"
            || tcp_client.max_clients != 1
            || tcp_client.default_port == 0
            || tcp_client.hostname_max_bytes != crate::CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES
    }) {
        return Err(ManifestError::CatalogMismatch {
            board: board.to_string(),
            field: "TCP client provisioning contract is unsupported".to_string(),
        });
    }
    Ok(ProvisioningSlot {
        wire_format: ProvisioningFormat::parse(&slot.format).map_err(|error| {
            ManifestError::CatalogMismatch {
                board: board.to_string(),
                field: error.to_string(),
            }
        })?,
        wire_format_version: slot.version,
        flash_offset: slot.offset,
        reserved_size_bytes: slot.size,
        ssid_max_bytes: slot.ssid_max_bytes,
        password_max_bytes: slot.password_max_bytes,
        tcp_client: slot.tcp_client,
    })
}

fn release_domain_error(error: impl fmt::Display) -> ManifestError {
    ManifestError::Release(error.to_string())
}

fn invalid_part_values(board: &str, path: &str, message: &str) -> ManifestError {
    ManifestError::InvalidPart {
        board: board.to_string(),
        path: path.to_string(),
        message: message.to_string(),
    }
}
