use std::io::{Read, Write};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MANIFEST_LIMIT: usize = 4096;
const SIGNATURE_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Board {
    #[serde(rename = "morse,ekh03v3")]
    ThinkNodeG4,
    #[serde(rename = "Heltec,HT-HD01-V2")]
    HeltecHtHd01V2,
}

impl Board {
    pub fn from_vendor_name(name: &str) -> Result<Self, PackageError> {
        match name.trim() {
            "morse,ekh03v3" => Ok(Self::ThinkNodeG4),
            "Heltec,HT-HD01-V2" => Ok(Self::HeltecHtHd01V2),
            _ => Err(PackageError::UnsupportedBoard),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Budgets {
    pub compressed_bytes: u64,
    pub executable_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    version: String,
    board: Board,
    abi: Abi,
    compressed: Artifact,
    executable: Artifact,
}

#[derive(Debug, Deserialize)]
enum Abi {
    #[serde(rename = "mips32r2-le-o32-soft-float-static")]
    Mips32r2LeO32SoftFloatStatic,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("unsupported vendor board")]
    UnsupportedBoard,
    #[error("package exceeds the configured budget")]
    Budget,
    #[error("invalid application manifest: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("unsupported application manifest schema")]
    Schema,
    #[error("invalid release version or digest")]
    Domain,
    #[error("package targets another board")]
    BoardMismatch,
    #[error("release signature refused: {0}")]
    Signature(#[from] prns_flash_manifest::TrustError),
    #[error("artifact length or hash differs from the signed manifest")]
    ArtifactMismatch,
    #[error("not a static MIPS32r2 little-endian O32 soft-float executable")]
    Abi,
    #[error("package I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub struct VerifiedPackage {
    pub(crate) manifest: Vec<u8>,
    pub(crate) signature: String,
    pub(crate) compressed: Vec<u8>,
    executable: Artifact,
}

impl VerifiedPackage {
    pub fn verify(
        manifest: Vec<u8>,
        signature: String,
        compressed: Vec<u8>,
        public_key: &str,
        board: &Board,
        budgets: &Budgets,
    ) -> Result<Self, PackageError> {
        if manifest.len() > MANIFEST_LIMIT
            || signature.len() > SIGNATURE_LIMIT
            || compressed.len() as u64 > budgets.compressed_bytes
        {
            return Err(PackageError::Budget);
        }
        prns_flash_manifest::verify_minisign(&manifest, &signature, public_key)?;
        let parsed: Manifest = serde_json::from_slice(&manifest)?;
        if parsed.schema != 1 {
            return Err(PackageError::Schema);
        }
        prns_flash_manifest::ReleaseVersion::parse(&parsed.version)
            .map_err(|_| PackageError::Domain)?;
        for artifact in [&parsed.compressed, &parsed.executable] {
            prns_flash_manifest::Sha256Digest::parse(&artifact.sha256)
                .map_err(|_| PackageError::Domain)?;
            if artifact.bytes == 0 {
                return Err(PackageError::Domain);
            }
        }
        if &parsed.board != board {
            return Err(PackageError::BoardMismatch);
        }
        if parsed.compressed.bytes > budgets.compressed_bytes
            || parsed.executable.bytes > budgets.executable_bytes
        {
            return Err(PackageError::Budget);
        }
        let _abi = parsed.abi;
        if compressed.len() as u64 != parsed.compressed.bytes
            || prns_flash_manifest::sha256_hex(&compressed) != parsed.compressed.sha256
        {
            return Err(PackageError::ArtifactMismatch);
        }
        let package = Self {
            manifest,
            signature,
            compressed,
            executable: parsed.executable,
        };
        package.expand(&mut std::io::sink())?;
        Ok(package)
    }

    pub fn executable_digest(&self) -> &str {
        &self.executable.sha256
    }
    pub(crate) fn executable_bytes(&self) -> u64 {
        self.executable.bytes
    }

    pub fn expand(&self, output: &mut impl Write) -> Result<(), PackageError> {
        let mut decoder = GzDecoder::new(self.compressed.as_slice());
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 8192];
        let mut header = Vec::new();
        loop {
            let count = decoder.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(PackageError::Budget)?;
            if total > self.executable.bytes {
                return Err(PackageError::ArtifactMismatch);
            }
            if header.len() < 65536 {
                header.extend_from_slice(&buffer[..count.min(65536 - header.len())]);
            }
            hasher.update(&buffer[..count]);
            output.write_all(&buffer[..count])?;
        }
        if total != self.executable.bytes
            || hex::encode(hasher.finalize()) != self.executable.sha256
        {
            return Err(PackageError::ArtifactMismatch);
        }
        verify_abi(&header)?;
        Ok(())
    }
}

fn verify_abi(data: &[u8]) -> Result<(), PackageError> {
    let word = |offset: usize| -> Result<u32, PackageError> {
        let bytes = data.get(offset..offset + 4).ok_or(PackageError::Abi)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().map_err(|_| PackageError::Abi)?,
        ))
    };
    let half = |offset: usize| -> Result<u16, PackageError> {
        let bytes = data.get(offset..offset + 2).ok_or(PackageError::Abi)?;
        Ok(u16::from_le_bytes(
            bytes.try_into().map_err(|_| PackageError::Abi)?,
        ))
    };
    if data.get(..7) != Some(b"\x7fELF\x01\x01\x01".as_slice())
        || half(16)? != 2
        || half(18)? != 8
        || word(20)? != 1
        || half(40)? != 52
        || word(36)? & 0xf000f000 != 0x70001000
    {
        return Err(PackageError::Abi);
    }
    let start = word(28)? as usize;
    let count = half(44)? as usize;
    if half(42)? != 32
        || count == 0
        || start < 52
        || start
            .checked_add(32 * count)
            .is_none_or(|end| end > data.len())
    {
        return Err(PackageError::Abi);
    }
    let mut load_segments = 0;
    for index in 0..count {
        match word(start + 32 * index)? {
            1 => load_segments += 1,
            2 | 3 => return Err(PackageError::Abi),
            _ => {}
        }
    }
    if load_segments == 0 {
        return Err(PackageError::Abi);
    }
    // EF_MIPS_FP64 and the GNU attributes would not suffice to prove soft-float.
    // Require the linker-provided .MIPS.abiflags segment and its FP ABI byte.
    for index in 0..count {
        let ph = start + 32 * index;
        if word(ph)? == 0x70000003 {
            let offset = word(ph + 4)? as usize;
            if data.get(offset + 7) == Some(&3) {
                return Ok(());
            }
            return Err(PackageError::Abi);
        }
    }
    Err(PackageError::Abi)
}
