//! chronicle-wal: the write-ahead log, snapshots, and recovery for
//! chronicle-core (ARCHITECTURE.md sections 9 and 10).
//!
//! The log persists accepted commands, never events, as CRC32C
//! framed records with a 32 byte header and fixed width payloads.
//! The reader implements the section 9.5 decision table, so a torn
//! tail from a crash is a reported boundary and anything worse is a
//! typed corruption error at an exact offset. Snapshots are a
//! structural dump of the book saved by an atomic rename protocol
//! and validated field by field on load. Recovery rebuilds a book
//! from a snapshot plus the log tail and proves out by state
//! equality with the live run.
//!
//! This crate forbids unsafe code. Its recovery path reads no clock,
//! uses no randomness, and iterates no hash maps, so recovery is as
//! deterministic as the book it rebuilds.
#![forbid(unsafe_code)]

mod crc;
mod error;
mod frame;
mod reader;
mod recover;
mod snapshot;
mod writer;

pub use crc::{Crc32c, crc32c};
pub use error::{SnapshotCorruption, WalCorruption, WalError};
pub use frame::{FrameParse, HEADER_LEN, MAX_PAYLOAD_LEN, WalCommand, encode_frame, parse_frame};
pub use reader::{ScanOutcome, ScannedFrame, read_log, scan_log};
pub use recover::{Recovery, RecoveryReport, recover, recover_from_files};
pub use snapshot::{
    SaveStep, decode_snapshot, encode_snapshot, encoded_len, load_snapshot, save_snapshot,
    save_snapshot_with_hook,
};
pub use writer::{SyncPolicy, WalSink, WalWriter};
