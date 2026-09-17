//! Positional byte source shared by journal scanning and ZMS decoding.
use std::io;

/// A byte source that supports exact positional reads.
pub trait ReadAt {
    /// Reads exactly `buf.len()` bytes starting at `offset`.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::UnexpectedEof`] when fewer than `buf.len()` bytes
    /// are available at `offset`.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;

    /// Total length of the source in bytes.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the length cannot be determined (e.g. `stat` fails).
    fn byte_len(&self) -> io::Result<u64>;
}

#[cfg(unix)]
impl ReadAt for std::fs::File {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, offset)
    }
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

impl ReadAt for &[u8] {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start =
            usize::try_from(offset).map_err(|_e| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        if end > self.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.copy_from_slice(&self[start..end]);
        Ok(())
    }
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

impl ReadAt for Vec<u8> {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.as_slice().read_exact_at(buf, offset)
    }
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

#[cfg(test)]
#[path = "tests/read_at.rs"]
mod tests;
