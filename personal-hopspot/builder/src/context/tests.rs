use super::*;

#[test]
fn developer_context_owns_validated_identity_and_paths() -> Result<(), BuildError> {
    let repository = Path::new("/repository");
    let output = Path::new("/artifacts");
    let digest = "e3ffc728180a8194c2efb55f90b0285f093db6e53e6dc800d4b229426e966399";
    let version = format!("0.3.7-dev.dirty.{digest}");
    let context = BuildContext::new(repository, output, BuildVersion::Developer(&version))?;

    assert_eq!(context.repository(), repository);
    assert_eq!(context.output_root(), output);
    assert_eq!(context.version(), version);
    assert_eq!(context.source_digest(), Some(digest));
    assert_eq!(
        context.board_output("t-echo"),
        Path::new("/artifacts/firmware/hopspot/t-echo").join(&version)
    );
    assert_eq!(
        context.work_output("t-echo"),
        Path::new("/repository/target/flash-artifacts/work/t-echo")
    );
    assert_eq!(
        context.release_part_path("t-echo", "application.uf2"),
        format!("firmware/hopspot/t-echo/{version}/application.uf2")
    );
    Ok(())
}

#[test]
fn release_artifact_root_is_repository_scoped() {
    assert_eq!(
        default_artifact_root(Path::new("/repository")),
        Path::new("/repository/target/flash-artifacts")
    );
}

#[test]
fn release_context_preserves_cargo_and_omits_linker_maps() -> Result<(), BuildError> {
    let context = BuildContext::new(
        Path::new("/repository"),
        Path::new("/artifacts"),
        BuildVersion::Developer("0.3.7"),
    )?;

    assert_eq!(context.intent(), BuildIntent::default());
    assert_eq!(context.cargo_subcommand(), "build");
    assert_eq!(context.linker_map_path("t114"), None);
    Ok(())
}

#[test]
fn evidence_context_separates_linker_maps_by_lto_mode() -> Result<(), BuildError> {
    let context = BuildContext::new(
        Path::new("/repository"),
        Path::new("/artifacts"),
        BuildVersion::Developer("0.3.7"),
    )?
    .with_intent(BuildIntent::ResourceReport {
        lto: crate::LtoMode::Thin,
    });

    assert_eq!(context.cargo_subcommand(), "rustc");
    assert_eq!(
        context.linker_map_path("t114"),
        Some(PathBuf::from("/artifacts/thin/work/t114/linker.map"))
    );
    assert_eq!(
        context.cargo_target_directory("t114"),
        Some(PathBuf::from("/artifacts/thin/work/t114/cargo"))
    );
    let pending = context.pending_linker_map_path("t114");
    assert!(pending.as_ref().is_some_and(|path| {
        path.parent() == Some(Path::new("/artifacts/thin/work/t114"))
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy().starts_with("linker.")
                    && name.to_string_lossy().ends_with(".map")
            })
    }));
    assert_eq!(
        context.board_output("t114"),
        PathBuf::from("/artifacts/thin/firmware/hopspot/t114/0.3.7")
    );
    Ok(())
}

#[test]
fn invalid_developer_versions_never_form_contexts() {
    assert!(matches!(
        BuildContext::new(
            Path::new("/repository"),
            Path::new("/artifacts"),
            BuildVersion::Developer("../invalid")
        ),
        Err(BuildError::Repository(_))
    ));
}
