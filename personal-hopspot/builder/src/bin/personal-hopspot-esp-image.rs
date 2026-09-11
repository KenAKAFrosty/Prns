use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use personal_hopspot_builder::platform::esp::{merged_flash_image, MergedImageRequest};
use personal_hopspot_builder::BuildError;
use prns_flash_manifest::{board_catalog, BoardBuild, CatalogError};
use thiserror::Error;

#[derive(Debug, Error)]
enum ImageError {
    #[error("missing {0}")]
    MissingArgument(&'static str),
    #[error("{0} is not valid UTF-8")]
    NonUtf8(&'static str),
    #[error("expected ELF, board, repository, and output")]
    UnexpectedArgument,
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("unknown ESP board {0:?}")]
    UnknownBoard(String),
    #[error("board {0:?} does not use the ESP build pipeline")]
    NotEspBoard(String),
    #[error("ESP board {0:?} has no flash size")]
    MissingFlashSize(String),
    #[error("could not read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Package(#[from] BuildError),
    #[error("could not write {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ESP_IMAGE_ERROR: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), ImageError> {
    let mut arguments = env::args_os().skip(1);
    let elf = PathBuf::from(required(&mut arguments, "ELF")?);
    let board_slug = text(required(&mut arguments, "board")?, "board")?;
    let repository = PathBuf::from(required(&mut arguments, "repository")?);
    let output = PathBuf::from(required(&mut arguments, "output")?);
    if arguments.next().is_some() {
        return Err(ImageError::UnexpectedArgument);
    }
    let elf_bytes = fs::read(&elf).map_err(|source| ImageError::Read { path: elf, source })?;
    let catalog = board_catalog()?;
    let board = catalog
        .board(&board_slug)
        .ok_or_else(|| ImageError::UnknownBoard(board_slug.clone()))?;
    let BoardBuild::Esp(recipe) = &board.build else {
        return Err(ImageError::NotEspBoard(board_slug));
    };
    let flash_size = board
        .flash_size
        .ok_or_else(|| ImageError::MissingFlashSize(board.slug.clone()))?;
    let partition_table = repository
        .join("personal-hopspot")
        .join("embedded")
        .join("esp32")
        .join(&recipe.partition_table);
    let image = merged_flash_image(MergedImageRequest {
        elf: &elf_bytes,
        partition_table: &partition_table,
        chip: &recipe.chip,
        flash_size_bytes: flash_size,
    })?;
    fs::write(&output, image).map_err(|source| ImageError::Write {
        path: output,
        source,
    })
}

fn required(
    arguments: &mut impl Iterator<Item = OsString>,
    name: &'static str,
) -> Result<OsString, ImageError> {
    arguments.next().ok_or(ImageError::MissingArgument(name))
}

fn text(value: OsString, name: &'static str) -> Result<String, ImageError> {
    value.into_string().map_err(|_| ImageError::NonUtf8(name))
}
