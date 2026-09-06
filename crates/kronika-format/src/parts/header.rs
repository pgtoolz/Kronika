//! The journal root header and its reset marker.

use super::{
    Error, JOURNAL_HEADER_LEN, JOURNAL_MAGIC, JOURNAL_VERSION, RESET_MARKER_LEN,
    RESET_MARKER_MAGIC, crc32c, fmt,
};

/// Hard cap on candidate frame headers examined while resynchronizing one
/// damaged journal generation.
/// Whether a version-1 journal is empty or belongs to one active segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    /// No frames and no segment identity.
    Empty,
    /// One or more frames belonging to the stored segment identity.
    Active {
        /// Unix microseconds of the first successfully appended window.
        segment_id: i64,
    },
}

/// Versioned root header of `active.wal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalHeader {
    /// Empty or active segment state.
    pub state: JournalState,
    /// Exact number of frame bytes following this header.
    pub body_len: u64,
}

impl JournalHeader {
    /// Canonical empty journal header.
    pub const EMPTY: Self = Self {
        state: JournalState::Empty,
        body_len: 0,
    };

    /// Encodes the complete checksummed 36-byte header.
    #[must_use]
    pub fn encode(self) -> [u8; JOURNAL_HEADER_LEN] {
        let mut bytes = [0_u8; JOURNAL_HEADER_LEN];
        bytes[..8].copy_from_slice(&JOURNAL_MAGIC);
        bytes[8..12].copy_from_slice(&JOURNAL_VERSION.to_le_bytes());
        match self.state {
            JournalState::Empty => {}
            JournalState::Active { segment_id } => {
                bytes[12] = 1;
                bytes[13] = 1;
                bytes[16..24].copy_from_slice(&segment_id.to_le_bytes());
            }
        }
        bytes[24..32].copy_from_slice(&self.body_len.to_le_bytes());
        let checksum = crc32c(&bytes[..32]);
        bytes[32..36].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Decodes and validates a complete version-1 journal header.
    ///
    /// # Errors
    ///
    /// Returns a specific [`JournalHeaderError`] for incompatible magic or
    /// version, checksum damage, invalid state, or missing identity.
    pub fn decode(bytes: [u8; JOURNAL_HEADER_LEN]) -> Result<Self, JournalHeaderError> {
        if bytes[..8] != JOURNAL_MAGIC {
            let mut actual = [0_u8; 8];
            actual.copy_from_slice(&bytes[..8]);
            return Err(JournalHeaderError::UnsupportedMagic { actual });
        }
        let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if version != JOURNAL_VERSION {
            return Err(JournalHeaderError::UnsupportedVersion { version });
        }
        let stored = u32::from_le_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);
        let computed = crc32c(&bytes[..32]);
        if stored != computed {
            return Err(JournalHeaderError::BadChecksum { stored, computed });
        }
        if bytes[14] != 0 || bytes[15] != 0 {
            return Err(JournalHeaderError::NonZeroReserved);
        }
        let id_present = bytes[13];
        let segment_id = i64::from_le_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]);
        let body_len = u64::from_le_bytes([
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        ]);
        let state = match (bytes[12], id_present) {
            (0, 0) => {
                if segment_id != 0 {
                    return Err(JournalHeaderError::UnexpectedIdentity);
                }
                JournalState::Empty
            }
            (1, 1) => JournalState::Active { segment_id },
            (1, 0) => return Err(JournalHeaderError::MissingIdentity),
            (0, 1) => return Err(JournalHeaderError::UnexpectedIdentity),
            (state, _) => return Err(JournalHeaderError::InvalidState { state }),
        };
        Ok(Self { state, body_len })
    }
}

/// Why a complete active-journal header is not a valid version-1 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalHeaderError {
    /// Magic identifies an unrelated or malformed file.
    UnsupportedMagic {
        /// Bytes found at the start of the file.
        actual: [u8; 8],
    },
    /// The file declares another journal version.
    UnsupportedVersion {
        /// Version found in the file.
        version: u32,
    },
    /// Header checksum does not match.
    BadChecksum {
        /// Stored checksum.
        stored: u32,
        /// Computed checksum.
        computed: u32,
    },
    /// State byte is not defined.
    InvalidState {
        /// State byte found in the file.
        state: u8,
    },
    /// Active state does not mark its segment identity as present.
    MissingIdentity,
    /// Empty state unexpectedly carries a segment identity.
    UnexpectedIdentity,
    /// Reserved bytes are non-zero.
    NonZeroReserved,
}

impl fmt::Display for JournalHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMagic { actual } => {
                write!(f, "unsupported active-journal magic {actual:02x?}")
            }
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported active-journal version {version}")
            }
            Self::BadChecksum { stored, computed } => write!(
                f,
                "active-journal header crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
            ),
            Self::InvalidState { state } => {
                write!(f, "invalid active-journal state {state}")
            }
            Self::MissingIdentity => {
                f.write_str("active journal state is missing its segment identity")
            }
            Self::UnexpectedIdentity => {
                f.write_str("empty journal state unexpectedly carries a segment identity")
            }
            Self::NonZeroReserved => f.write_str("active-journal reserved bytes are non-zero"),
        }
    }
}

impl Error for JournalHeaderError {}

/// Durable intent to replace one active journal generation with the canonical
/// empty header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_field_names,
    reason = "the shared on-disk fields explicitly describe the previous journal generation"
)]
pub struct ResetMarker {
    /// Complete journal length before the marker was appended.
    pub previous_len: u64,
    /// Segment identity stored by the previous active header.
    pub previous_segment_id: i64,
    /// CRC32C of the complete encoded previous active header.
    pub previous_header_crc: u32,
}

impl ResetMarker {
    /// Builds a marker for one non-empty active journal generation.
    #[must_use]
    pub fn new(previous_len: u64, previous_segment_id: i64) -> Option<Self> {
        if previous_len <= JOURNAL_HEADER_LEN as u64 {
            return None;
        }
        let header = JournalHeader {
            state: JournalState::Active {
                segment_id: previous_segment_id,
            },
            body_len: previous_len - JOURNAL_HEADER_LEN as u64,
        };
        Some(Self {
            previous_len,
            previous_segment_id,
            previous_header_crc: crc32c(&header.encode()),
        })
    }

    /// Encodes the checksummed marker.
    #[must_use]
    pub fn encode(self) -> [u8; RESET_MARKER_LEN] {
        let mut bytes = [0_u8; RESET_MARKER_LEN];
        bytes[..8].copy_from_slice(&RESET_MARKER_MAGIC);
        bytes[8..16].copy_from_slice(&self.previous_len.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.previous_segment_id.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.previous_header_crc.to_le_bytes());
        let checksum = crc32c(&bytes[..28]);
        bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    /// Decodes a marker after checking its magic and checksum.
    #[must_use]
    pub fn decode(bytes: [u8; RESET_MARKER_LEN]) -> Option<Self> {
        if bytes[..8] != RESET_MARKER_MAGIC {
            return None;
        }
        let stored = u32::from_le_bytes(bytes[28..32].try_into().ok()?);
        if stored != crc32c(&bytes[..28]) {
            return None;
        }
        Some(Self {
            previous_len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            previous_segment_id: i64::from_le_bytes(bytes[16..24].try_into().ok()?),
            previous_header_crc: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
        })
    }

    /// Reconstructs and verifies the active header named by this marker.
    #[must_use]
    pub fn expected_previous_header(self) -> Option<JournalHeader> {
        let marker = Self::new(self.previous_len, self.previous_segment_id)?;
        (marker.previous_header_crc == self.previous_header_crc).then_some(JournalHeader {
            state: JournalState::Active {
                segment_id: self.previous_segment_id,
            },
            body_len: self.previous_len - JOURNAL_HEADER_LEN as u64,
        })
    }

    /// Classifies an observed root header during a committed reset.
    ///
    /// Besides either complete header, only a prefix rewrite from the previous
    /// active bytes to the empty bytes is accepted. Unrelated corruption and
    /// non-v1 headers are not reset transitions.
    #[must_use]
    pub fn classify_header_transition(
        self,
        observed: [u8; JOURNAL_HEADER_LEN],
    ) -> Option<ResetHeaderTransition> {
        let previous = self.expected_previous_header()?.encode();
        let empty = JournalHeader::EMPTY.encode();
        if observed == previous {
            return Some(ResetHeaderTransition::Previous);
        }
        if observed == empty {
            return Some(ResetHeaderTransition::Empty);
        }
        (1..JOURNAL_HEADER_LEN)
            .any(|split| {
                observed[..split] == empty[..split] && observed[split..] == previous[split..]
            })
            .then_some(ResetHeaderTransition::Torn)
    }
}

/// Valid root-header states observed after a reset marker was committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetHeaderTransition {
    /// The previous active header is still complete.
    Previous,
    /// The canonical empty header is already complete.
    Empty,
    /// A prefix of the empty header replaced the corresponding active bytes.
    Torn,
}
