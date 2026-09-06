//! Capped byte counting for encoded requests.

/// An `io::Write` that stops once an encoded byte limit is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ByteBudget {
    remaining: usize,
}

impl ByteBudget {
    /// Starts a counter with `limit` writable bytes.
    pub(crate) const fn new(limit: usize) -> Self {
        Self { remaining: limit }
    }
}

impl std::io::Write for ByteBudget {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("over budget"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
