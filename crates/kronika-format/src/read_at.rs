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
mod tests {
    use super::ReadAt;
    #[test]
    fn slice_reads_at_offset_and_reports_len() {
        let data: &[u8] = b"0123456789";
        assert_eq!(ReadAt::byte_len(&data).unwrap(), 10);
        let mut buf = [0_u8; 3];
        data.read_exact_at(&mut buf, 4).unwrap();
        assert_eq!(&buf, b"456");
    }
    #[test]
    fn slice_read_past_end_errors() {
        let data: &[u8] = b"abc";
        let mut buf = [0_u8; 4];
        assert!(data.read_exact_at(&mut buf, 0).is_err());
        assert!(data.read_exact_at(&mut buf, 3).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn file_reads_at_offset() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        let file = std::fs::File::open(f.path()).unwrap();
        assert_eq!(ReadAt::byte_len(&file).unwrap(), 11);
        let mut buf = [0_u8; 5];
        file.read_exact_at(&mut buf, 6).unwrap();
        assert_eq!(&buf, b"world");
    }
    #[test]
    fn vec_reads_at_offset_and_reports_len() {
        let data: Vec<u8> = b"0123456789".to_vec();
        assert_eq!(data.byte_len().unwrap(), 10);
        let mut buf = [0_u8; 3];
        data.read_exact_at(&mut buf, 4).unwrap();
        assert_eq!(&buf, b"456");
    }
    #[test]
    fn vec_read_past_end_errors() {
        let data: Vec<u8> = b"abc".to_vec();
        let mut buf = [0_u8; 4];
        assert!(data.read_exact_at(&mut buf, 0).is_err());
        assert!(data.read_exact_at(&mut buf, 3).is_err());
    }
}
