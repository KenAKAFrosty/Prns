//! Compressed-section decoding and malformed Zstandard input checks.

use object::read::{CompressedData, CompressionFormat};

fn section(data: &[u8], size: u64) -> CompressedData<'_> {
    CompressedData {
        format: CompressionFormat::Zstandard,
        data,
        uncompressed_size: size,
    }
}

#[test]
fn reads_concatenated_zstd_frames_and_skips_metadata() -> Result<(), Box<dyn std::error::Error>> {
    // Two single-segment raw frames containing "ab" and "cd", separated by a
    // skippable frame. These are complete format fixtures, not encoder output.
    let data = [
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 2, 0x11, 0, 0, b'a', b'b', 0x50, 0x2a, 0x4d, 0x18, 2, 0, 0,
        0, 0xff, 0xff, 0x28, 0xb5, 0x2f, 0xfd, 0x20, 2, 0x11, 0, 0, b'c', b'd',
    ];
    assert_eq!(section(&data, 4).decompress()?.as_ref(), b"abcd");
    Ok(())
}

#[test]
fn accepts_zstd_block_at_the_window_bound() -> Result<(), Box<dyn std::error::Error>> {
    // A 1 KiB window, no declared content size, one last RLE block of 1,024 bytes.
    let data = [0x28, 0xb5, 0x2f, 0xfd, 0, 0, 0x03, 0x20, 0, b'a'];
    assert_eq!(section(&data, 1024).decompress()?.as_ref(), &[b'a'; 1024]);
    Ok(())
}

#[test]
fn rejects_zstd_block_larger_than_its_window() {
    // Same frame with a 1,025-byte block. The old decoder accepted this; the
    // upstream RUSTSEC-2026-0344 fix enforces min(window size, 128 KiB) per block.
    // At most 1,025 output bytes makes this regression safe on the old version.
    let data = [0x28, 0xb5, 0x2f, 0xfd, 0, 0, 0x0b, 0x20, 0, b'a'];
    assert!(section(&data, 1025).decompress().is_err());
}

#[test]
fn rejects_truncated_zstd_section() {
    let data = [0x28, 0xb5, 0x2f, 0xfd, 0x20, 2, 0x11, 0, 0, b'a'];
    assert!(section(&data, 2).decompress().is_err());
}
