#![forbid(unsafe_code)]

//! Offline development enrollment. Arguments contain public identities only.
use prns_groups::fixture;
use prns_groups_core::Timestamp;
use prns_host::IdentityHash;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

#[derive(Debug)]
enum Error {
    Usage,
    Identity,
    Clock,
    Io(std::io::Error),
    Fixture(fixture::FixtureError),
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Usage => f.write_str("usage: dev-group-fixture NEW_OUTPUT_DIRECTORY TITLE RETICULUM_IDENTITY_HEX RETICULUM_IDENTITY_HEX [...]; identities are 16-byte public hashes"),
            Self::Identity => f.write_str("invalid public Reticulum identity hash"),
            Self::Clock => f.write_str("system time is before the Unix epoch"),
            Self::Io(error) => write!(f, "fixture file: {error}"),
            Self::Fixture(error) => core::fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Error> {
    let mut arguments = std::env::args().skip(1);
    let directory = PathBuf::from(arguments.next().ok_or(Error::Usage)?);
    let title = arguments.next().ok_or(Error::Usage)?;
    let identities = arguments
        .map(|argument| parse_identity(&argument))
        .collect::<Result<Vec<_>, _>>()?;
    if identities.len() < 2 {
        return Err(Error::Usage);
    }
    let now = Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Clock)?
            .as_secs(),
    );
    let fixtures = fixture::generate(&title, &identities, now).map_err(Error::Fixture)?;
    fs::create_dir(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    for (index, fixture) in fixtures.into_iter().enumerate() {
        let bytes = Zeroizing::new(fixture);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let path = directory.join(format!("member-{}.json", index + 1));
        let mut file = options.open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        println!("{}", path.display());
    }
    println!("Private development fixtures created. Install each on exactly its assigned device; do not clone or publish them.");
    Ok(())
}
fn parse_identity(text: &str) -> Result<IdentityHash, Error> {
    if text.len() != 32 {
        return Err(Error::Identity);
    }
    let mut bytes = [0; 16];
    for (index, pair) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair = core::str::from_utf8(pair).map_err(|_| Error::Identity)?;
        bytes[index] = u8::from_str_radix(pair, 16).map_err(|_| Error::Identity)?;
    }
    Ok(IdentityHash::new(bytes))
}
