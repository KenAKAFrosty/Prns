use std::path::Path;
use std::process::Command;

use personal_hopspot_builder::architecture::Adapter;

use super::architecture::AssuranceAdapter;
use super::{DisassemblyAnalysis, ExecutableError, ExecutableSection};

pub(super) fn analyze(
    path: &Path,
    build_adapter: &Adapter,
    assurance_adapter: &AssuranceAdapter,
    sections: &[ExecutableSection],
    entry_point: u64,
) -> Result<DisassemblyAnalysis, ExecutableError> {
    let tool = build_adapter.resolve_disassembler().map_err(|source| {
        ExecutableError::ResolveDisassembler {
            architecture: assurance_adapter.id(),
            source,
        }
    })?;
    let version = version(&tool, path)?;
    let output = disassemble(&tool, path)?;
    let decoded = output
        .lines()
        .filter_map(|line| assurance_adapter.decoded_instruction(line))
        .collect::<Vec<_>>();
    if decoded.is_empty() {
        return Err(ExecutableError::EmptyDisassembly {
            program: tool.program(),
            path: path.to_path_buf(),
        });
    }
    let normalized_entry = assurance_adapter.normalize_code_address(entry_point);
    if !decoded.iter().any(|instruction| {
        instruction.address <= normalized_entry && normalized_entry < instruction.end()
    }) {
        return Err(ExecutableError::UndecodedEntry {
            program: tool.program(),
            path: path.to_path_buf(),
            entry: entry_point,
        });
    }
    let executable_bytes = sections
        .iter()
        .try_fold(0_u64, |total, section| {
            total.checked_add(section.range.byte_len())
        })
        .ok_or(ExecutableError::CountOverflow {
            evidence: "executable section bytes",
        })?;
    let decoded_bytes = super::coverage::covered_bytes(
        decoded
            .iter()
            .filter(|instruction| {
                sections
                    .iter()
                    .any(|section| section.range.contains(instruction.range()))
            })
            .map(|instruction| instruction.range())
            .collect(),
        "decoded instruction bytes",
    )?;
    let instruction_count =
        u64::try_from(decoded.len()).map_err(|_| ExecutableError::CountOverflow {
            evidence: "decoded instruction count",
        })?;

    Ok(DisassemblyAnalysis {
        adapter: build_adapter.architecture(),
        flavor: tool.flavor(),
        program: tool.program(),
        version: version_identity(&version),
        executable_bytes,
        decoded_bytes,
        undecoded_bytes: executable_bytes - decoded_bytes,
        instruction_count,
        instructions: decoded,
    })
}

fn version_identity(output: &str) -> String {
    output
        .lines()
        .find(|line| line.to_ascii_lowercase().contains("version"))
        .or_else(|| output.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn version(
    tool: &personal_hopspot_builder::architecture::ResolvedDisassembler,
    path: &Path,
) -> Result<String, ExecutableError> {
    let output = Command::new(tool.path())
        .args(tool.version_arguments())
        .output()
        .map_err(|source| ExecutableError::RunDisassembler {
            program: tool.program(),
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(ExecutableError::DisassemblerFailed {
            program: tool.program(),
            path: path.to_path_buf(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| ExecutableError::InvalidDisassembly {
        program: tool.program(),
        path: path.to_path_buf(),
        source,
    })
}

fn disassemble(
    tool: &personal_hopspot_builder::architecture::ResolvedDisassembler,
    path: &Path,
) -> Result<String, ExecutableError> {
    let output = Command::new(tool.path())
        .args(["-d", "-z"])
        .arg(path)
        .output()
        .map_err(|source| ExecutableError::RunDisassembler {
            program: tool.program(),
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(ExecutableError::DisassemblerFailed {
            program: tool.program(),
            path: path.to_path_buf(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| ExecutableError::InvalidDisassembly {
        program: tool.program(),
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::version_identity;

    #[test]
    fn version_identity_prefers_the_version_bearing_line() {
        assert_eq!(
            version_identity("LLVM (http://llvm.org/):\n  LLVM version 22.1.8\n"),
            "LLVM version 22.1.8"
        );
        assert_eq!(
            version_identity("GNU objdump (toolchain) 2.45\nCopyright\n"),
            "GNU objdump (toolchain) 2.45"
        );
    }
}
