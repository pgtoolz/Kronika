//! The offsets a restart resumes from.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::tail::Position;

/// Name of the file the collector keeps its log offsets in.
pub const OFFSETS_FILE_NAME: &str = "log.offsets";

/// The temporary the offsets file is renamed from.
pub const OFFSETS_TEMP_FILE_NAME: &str = "log.tmp";

/// Every followed file's resume point, keyed by source name.
///
/// One line per source, `name dev inode offset`. A line that does not parse is
/// dropped, which costs the lines written between the damaged write and the
/// restart and nothing else.
#[derive(Debug, Default)]
pub struct Offsets {
    path: PathBuf,
    positions: BTreeMap<String, Position>,
}

impl Offsets {
    /// Read `<dir>/log.offsets`, or start empty when it is not there yet.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error for reading the file, except the
    /// missing file of a first start.
    pub fn load(dir: &Path) -> io::Result<Self> {
        let path = dir.join(OFFSETS_FILE_NAME);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        Ok(Self {
            path,
            positions: parse(&text),
        })
    }

    /// Where `name` left off, or the start of a file it has never read.
    #[must_use]
    pub fn get(&self, name: &str) -> Position {
        self.positions.get(name).copied().unwrap_or_default()
    }

    /// Record where `name` left off. Call [`save`](Self::save) to persist it.
    pub fn set(&mut self, name: &str, position: Position) {
        self.positions.insert(name.to_owned(), position);
    }

    /// Write the file, replacing the previous one in a single rename.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error for writing, syncing or renaming
    /// the file.
    pub fn save(&self) -> io::Result<()> {
        let temp = self.path.with_file_name(OFFSETS_TEMP_FILE_NAME);
        let mut file = fs::File::create(&temp)?;
        file.write_all(render(&self.positions).as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &self.path)
    }
}

fn render(positions: &BTreeMap<String, Position>) -> String {
    let mut out = String::new();
    for (name, position) in positions {
        let _ = writeln!(
            out,
            "{name} {} {} {}",
            position.dev, position.inode, position.offset
        );
    }
    out
}

fn parse(text: &str) -> BTreeMap<String, Position> {
    let mut positions = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let parsed = (|| {
            let name = fields.next()?;
            let dev = fields.next()?.parse().ok()?;
            let inode = fields.next()?.parse().ok()?;
            let offset = fields.next()?.parse().ok()?;
            Some((name.to_owned(), Position { dev, inode, offset }))
        })();
        if let Some((name, position)) = parsed {
            positions.insert(name, position);
        }
    }
    positions
}

#[cfg(test)]
mod tests;
