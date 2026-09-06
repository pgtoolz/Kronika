//! Following log files and reading events out of them.
//!
//! [`Tail`] is the whole of the file handling: it follows a growing file
//! through a fixed buffer, survives truncation and rotation, cuts over-long
//! lines, drops what is not UTF-8, and remembers where it stopped. A log file's
//! size is set by someone else's software, so it is never held whole.
//!
//! Where one record ends and the next begins is not the tail's business. That
//! rule arrives as a [`Continues`] function from whichever parser is reading
//! the file: [`postgres`] for the three shapes `log_destination` produces,
//! [`pgbouncer`] for the one `PgBouncer` writes.

mod offsets;
mod tail;
mod text;
mod timestamp;

pub mod pgbouncer;
pub mod postgres;

pub use offsets::{OFFSETS_FILE_NAME, OFFSETS_TEMP_FILE_NAME, Offsets};
pub use tail::{Continues, MAX_LINE_BYTES, MAX_READ_BYTES, Position, Record, Tail, TailBatch};
pub use text::{MAX_PATTERN_BYTES, MAX_TEXT_BYTES};
