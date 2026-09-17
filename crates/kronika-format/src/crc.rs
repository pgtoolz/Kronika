//! CRC32C checksums for ZMS bytes.

use crc::{CRC_32_ISCSI, Crc};

/// Castagnoli CRC32C polynomial.
const CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);

/// Incremental Castagnoli CRC32C calculation for bounded streaming reads.
pub struct Crc32c {
    digest: crc::Digest<'static, u32>,
}

impl std::fmt::Debug for Crc32c {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Crc32c").finish_non_exhaustive()
    }
}

impl Crc32c {
    /// Starts an empty CRC32C calculation.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            digest: CRC32C.digest(),
        }
    }

    /// Adds the next contiguous byte chunk.
    pub const fn update(&mut self, bytes: &[u8]) {
        self.digest.update(bytes);
    }

    /// Completes the checksum.
    #[must_use]
    pub const fn finalize(self) -> u32 {
        self.digest.finalize()
    }
}

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the CRC32C checksum of `bytes`.
///
/// ZMS uses CRC32C for section bodies, the end catalog, and `active.wal`
/// frame headers.
#[must_use]
pub const fn crc32c(bytes: &[u8]) -> u32 {
    CRC32C.checksum(bytes)
}

/// Return the CRC32C of `bytes` with the four bytes at `zero_at` treated as
/// zeroes, without copying the input.
///
/// The end catalog stores its own checksum inside the checksummed range;
/// this computes the over-zeroed-field value incrementally instead of
/// cloning the whole block to blank the field.
pub(crate) fn crc32c_with_zeroed_field(bytes: &[u8], zero_at: usize) -> u32 {
    let mut digest = CRC32C.digest();
    digest.update(&bytes[..zero_at]);
    digest.update(&[0_u8; 4]);
    digest.update(&bytes[zero_at + 4..]);
    digest.finalize()
}

#[cfg(test)]
#[path = "tests/crc.rs"]
mod tests;
