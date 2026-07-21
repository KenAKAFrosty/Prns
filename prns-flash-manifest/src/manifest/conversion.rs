use std::fmt;

use crate::domain::TargetIdentity;
use crate::{
    AfterResetStrategy, BeforeResetStrategy, BoardCatalog, BoardCatalogEntry, BoardId, ChipFamily,
    EspFlashPart, EspSerialTarget, FlashFrequency, FlashMode, ImmutableArtifactPath, KeyId,
    PreparationProfile, ProvisioningDescriptor, ProvisioningFormat, ProvisioningSlot,
    ReleaseTarget, ReleaseVersion, Sha256Digest, Transport, Uf2Part, Uf2Target,
    ValidatedChannelDescriptor, ValidatedFlashManifest, ValidatedReleaseInfo, ValidatedSigningInfo,
    CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES, CONFIG_SIZE, CONFIG_SSID_MAX_BYTES, CONFIG_VERSION,
};

use super::validation::validate_target;
use super::{
    ChannelDescriptor, FlashManifest, FlashPart, FlashPartKind, ManifestError, ReleaseChannel,
    TargetManifest,
};

impl ValidatedFlashManifest {
    /// Parse schema-v2 JSON directly into the validated release-domain model.
    pub fn from_json(bytes: &[u8], catalog: &BoardCatalog) -> Result<Self, ManifestError> {
        let wire: FlashManifest = serde_json::from_slice(bytes)?;
        wire.into_validated(catalog)
    }
}

impl TargetManifest {
    /// Validate and consume one target record using its catalog entry and release version.
    pub fn into_validated(
        self,
        board: &BoardCatalogEntry,
        version: &ReleaseVersion,
    ) -> Result<ReleaseTarget, ManifestError> {
        validate_target(&self, board, version.as_str())?;
        convert_target(self)
    }
}

impl ValidatedChannelDescriptor {
    /// Parse a signed channel wire document into typed release and digest values.
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
        schema,
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
        validated_targets.push(convert_target(target)?);
    }
    Ok(ValidatedFlashManifest {
        schema,
        release: ValidatedReleaseInfo {
            version,
            channel: release.channel,
            commit: release.commit,
        },
        signing: ValidatedSigningInfo { key_id },
        targets: validated_targets,
    })
}

pub(super) fn convert_channel_descriptor(
    descriptor: ChannelDescriptor,
) -> Result<ValidatedChannelDescriptor, ManifestError> {
    Ok(ValidatedChannelDescriptor {
        schema: descriptor.schema,
        channel: descriptor.channel,
        version: ReleaseVersion::parse(descriptor.version).map_err(release_domain_error)?,
        manifest_url: descriptor.manifest_url,
        manifest_sha256: Sha256Digest::parse(descriptor.manifest_sha256)
            .map_err(release_domain_error)?,
    })
}

fn convert_target(target: TargetManifest) -> Result<ReleaseTarget, ManifestError> {
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
        provisioning,
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
    };
    match transport {
        Transport::EspSerial => {
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
                if part.kind == FlashPartKind::Uf2 {
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
            if identity.preparation_profile != PreparationProfile::TechoUf2 {
                return Err(ManifestError::CatalogMismatch {
                    board: board_slug,
                    field: "UF2 target requires the techo-uf2 preparation profile".to_string(),
                });
            }
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
            let [part] = <[FlashPart; 1]>::try_from(parts).map_err(|parts: Vec<FlashPart>| {
                invalid_part_values(
                    &board_slug,
                    "",
                    &format!("UF2 target requires one part, found {}", parts.len()),
                )
            })?;
            if part.kind != FlashPartKind::Uf2 || part.offset.is_some() {
                return Err(invalid_part_values(
                    &board_slug,
                    &part.path,
                    "UF2 target requires one offset-free UF2 part",
                ));
            }
            Ok(ReleaseTarget::Uf2(Uf2Target {
                identity,
                part: Uf2Part {
                    path: ImmutableArtifactPath::parse(part.path.clone()).map_err(|error| {
                        invalid_part_values(&board_slug, &part.path, &error.to_string())
                    })?,
                    size: part.size,
                    sha256: Sha256Digest::parse(part.sha256).map_err(|error| {
                        invalid_part_values(&board_slug, &part.path, &error.to_string())
                    })?,
                },
            }))
        }
    }
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
    Ok(ProvisioningSlot {
        format: ProvisioningFormat::parse(&slot.format).map_err(|error| {
            ManifestError::CatalogMismatch {
                board: board.to_string(),
                field: error.to_string(),
            }
        })?,
        version: slot.version,
        offset: slot.offset,
        size: slot.size,
        ssid_max_bytes: slot.ssid_max_bytes,
        password_max_bytes: slot.password_max_bytes,
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
