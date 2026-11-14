//! Typed failures for the write-ahead log, the snapshot format, and
//! recovery (ARCHITECTURE.md sections 9.5 and 10.4). Every corruption
//! error carries the offset or the field that failed, with the
//! expected and actual values where the format defines both, so a
//! caller can point at the exact byte range that is wrong. In the CRC
//! variants, `expected_crc` is the checksum computed over the bytes as
//! received (what the stored value should be for this content) and
//! `actual_crc` is the value stored in the file.

use std::fmt;
use std::io;

use chronicle_core::BookError;

/// What is wrong with one write-ahead log frame, at the offset named
/// by [`WalError::CorruptWalRecord`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalCorruption {
    /// The magic bytes are not `CWAL`.
    BadMagic,
    /// The frame version is not 1.
    BadVersion {
        /// The version value found in the header.
        found: u16,
    },
    /// The header length field is not 32.
    BadHeaderLen {
        /// The header length value found in the header.
        found: u16,
    },
    /// The payload length exceeds the 4096 byte cap.
    PayloadTooLarge {
        /// The payload length value found in the header.
        found: u32,
    },
    /// The payload length differs from the fixed width of its kind.
    PayloadLenMismatch {
        /// The frame kind byte.
        kind: u8,
        /// The fixed payload width the kind requires.
        expected: u32,
        /// The payload length value found in the header.
        found: u32,
    },
    /// The kind byte names no known command.
    UnknownKind {
        /// The kind byte found in the header.
        found: u8,
    },
    /// A field the format requires to be zero is not zero.
    NonZeroReserved {
        /// Name of the field, for example `flags` or `add.reserved`.
        field: &'static str,
    },
    /// A payload field holds a value outside its defined set, for
    /// example a side byte other than 0 or 1.
    BadPayloadField {
        /// Name of the field, for example `add.side`.
        field: &'static str,
    },
    /// The stored checksum does not match the received bytes.
    CrcMismatch {
        /// Checksum computed over the received bytes.
        expected_crc: u32,
        /// Checksum stored in the frame header.
        actual_crc: u32,
    },
}

/// What is wrong with a snapshot file, per the validation order of
/// ARCHITECTURE.md section 10.4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotCorruption {
    /// The file size differs from the size the header values imply.
    SizeMismatch {
        /// Size computed from the header values.
        expected: u64,
        /// Actual file size.
        actual: u64,
    },
    /// The magic bytes are not `CSNP`.
    BadMagic,
    /// The snapshot version is not 1.
    BadVersion {
        /// The version value found in the header.
        found: u16,
    },
    /// The header length field is not 64.
    BadHeaderLen {
        /// The header length value found in the header.
        found: u16,
    },
    /// A field the format requires to be zero is not zero.
    NonZeroReserved {
        /// Name of the field, for example `header.reserved`.
        field: &'static str,
    },
    /// A field holds a value outside its defined range or set.
    InvalidField {
        /// Name of the field, for example `live record side`.
        field: &'static str,
    },
    /// `next_seq` is not `last_included_seq + 1`.
    NextSeqMismatch {
        /// The last sequence number the snapshot includes.
        last_included_seq: u64,
        /// The next sequence number stored beside it.
        next_seq: u64,
    },
    /// A live slot's free list link field is not NIL.
    LiveSlotLinkNotNil {
        /// The slot whose link field is set.
        slot: u32,
    },
    /// A live record's generation disagrees with the generations
    /// section for its slot.
    GenerationMismatch {
        /// The slot whose generations disagree.
        slot: u32,
    },
    /// The persisted free list does not check out: it cycles, leaves
    /// the slot range, visits a live slot, or skips a free slot.
    FreeListInvalid {
        /// What the walk found wrong.
        reason: &'static str,
    },
    /// The stored trailer checksum does not match the received bytes.
    CrcMismatch {
        /// Checksum computed over the received bytes.
        expected_crc: u32,
        /// Checksum stored in the trailer.
        actual_crc: u32,
    },
}

/// Every failure the WAL, snapshot, and recovery paths can return.
#[derive(Debug)]
pub enum WalError {
    /// An underlying I/O failure, naming the operation that hit it.
    Io {
        /// The operation in progress, for example `read WAL file`.
        operation: &'static str,
        /// The I/O error itself.
        source: io::Error,
    },
    /// One frame is corrupt (section 9.5). Corruption is never a
    /// crash boundary and is never skipped.
    CorruptWalRecord {
        /// Byte offset of the frame's first byte in the log.
        offset: u64,
        /// What is wrong with the frame.
        detail: WalCorruption,
    },
    /// Frame sequence numbers are not consecutive (section 9.5).
    WalSequenceGap {
        /// The sequence number the next frame had to carry.
        expected: u64,
        /// The sequence number the frame actually carries.
        found: u64,
        /// Byte offset of the frame's first byte in the log.
        offset: u64,
    },
    /// A snapshot file fails validation (section 10.4). No partial
    /// state ever escapes a failed load.
    CorruptSnapshot {
        /// What is wrong with the snapshot.
        detail: SnapshotCorruption,
    },
    /// A snapshot was written for a different book configuration than
    /// the caller asked to recover into.
    SnapshotConfigMismatch {
        /// Name of the configuration field that differs.
        field: &'static str,
        /// The value stored in the snapshot (self-trade policies are
        /// reported by their frame encoding).
        snapshot_value: u64,
        /// The value in the caller's configuration.
        book_value: u64,
    },
    /// The book rejected a step of recovery: building the base book
    /// from a snapshot state (`seq` is `None`), or replaying one
    /// logged command (`seq` names the frame).
    Book {
        /// Sequence number of the frame being replayed, when the
        /// failure came from replay.
        seq: Option<u64>,
        /// The book's own error.
        source: BookError,
    },
}

impl WalError {
    /// Wrap an I/O error with the operation that hit it.
    pub(crate) fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl fmt::Display for WalCorruption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad magic, expected CWAL"),
            Self::BadVersion { found } => write!(f, "unsupported WAL version {found}"),
            Self::BadHeaderLen { found } => {
                write!(f, "header length {found}, expected 32")
            }
            Self::PayloadTooLarge { found } => {
                write!(f, "payload length {found} exceeds the 4096 byte cap")
            }
            Self::PayloadLenMismatch {
                kind,
                expected,
                found,
            } => write!(
                f,
                "payload length {found} does not match kind {kind} width {expected}"
            ),
            Self::UnknownKind { found } => write!(f, "unknown frame kind {found}"),
            Self::NonZeroReserved { field } => {
                write!(f, "reserved field {field} must be zero")
            }
            Self::BadPayloadField { field } => {
                write!(f, "payload field {field} holds an undefined value")
            }
            Self::CrcMismatch {
                expected_crc,
                actual_crc,
            } => write!(
                f,
                "checksum mismatch, computed {expected_crc:#010x}, stored {actual_crc:#010x}"
            ),
        }
    }
}

impl fmt::Display for SnapshotCorruption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeMismatch { expected, actual } => write!(
                f,
                "snapshot size {actual} does not match the header arithmetic {expected}"
            ),
            Self::BadMagic => write!(f, "bad magic, expected CSNP"),
            Self::BadVersion { found } => {
                write!(f, "unsupported snapshot version {found}")
            }
            Self::BadHeaderLen { found } => {
                write!(f, "header length {found}, expected 64")
            }
            Self::NonZeroReserved { field } => {
                write!(f, "reserved field {field} must be zero")
            }
            Self::InvalidField { field } => {
                write!(f, "field {field} holds an undefined value")
            }
            Self::NextSeqMismatch {
                last_included_seq,
                next_seq,
            } => write!(
                f,
                "next_seq {next_seq} is not last_included_seq {last_included_seq} plus one"
            ),
            Self::LiveSlotLinkNotNil { slot } => {
                write!(f, "live slot {slot} carries a free list link")
            }
            Self::GenerationMismatch { slot } => {
                write!(f, "generations disagree for slot {slot}")
            }
            Self::FreeListInvalid { reason } => write!(f, "free list invalid: {reason}"),
            Self::CrcMismatch {
                expected_crc,
                actual_crc,
            } => write!(
                f,
                "checksum mismatch, computed {expected_crc:#010x}, stored {actual_crc:#010x}"
            ),
        }
    }
}

impl fmt::Display for WalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation} failed: {source}"),
            Self::CorruptWalRecord { offset, detail } => {
                write!(f, "corrupt WAL record at offset {offset}: {detail}")
            }
            Self::WalSequenceGap {
                expected,
                found,
                offset,
            } => write!(
                f,
                "WAL sequence gap at offset {offset}: expected seq {expected}, found {found}"
            ),
            Self::CorruptSnapshot { detail } => write!(f, "corrupt snapshot: {detail}"),
            Self::SnapshotConfigMismatch {
                field,
                snapshot_value,
                book_value,
            } => write!(
                f,
                "snapshot configuration mismatch on {field}: snapshot has {snapshot_value}, book has {book_value}"
            ),
            Self::Book { seq, source } => match seq {
                Some(seq) => write!(f, "replay of seq {seq} failed: {source}"),
                None => write!(f, "book construction failed: {source}"),
            },
        }
    }
}

impl std::error::Error for WalCorruption {}
impl std::error::Error for SnapshotCorruption {}

impl std::error::Error for WalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Book { source, .. } => Some(source),
            _ => None,
        }
    }
}
