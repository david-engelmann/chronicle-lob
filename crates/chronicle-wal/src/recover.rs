//! Recovery (ARCHITECTURE.md section 10.4): rebuild a book from an
//! optional snapshot plus the write-ahead log. Frames the snapshot
//! already covers are skipped; the first replayed frame must carry
//! exactly `last_included_seq + 1`, and every frame after it exactly
//! one more, or recovery fails with [`WalError::WalSequenceGap`]. A
//! recovered book's `next_seq` is the highest recovered sequence plus
//! one, where the snapshot's `last_included_seq` counts when the log
//! adds nothing. The replay path is deterministic: no clock, no
//! randomness, no hash iteration, like everything it rebuilds.

use std::path::Path;

use chronicle_core::{Book, BookConfig, EventBuffer, ExecutionEvent, SnapshotState};

use crate::error::{SnapshotCorruption, WalError};
use crate::reader::scan_log;
use crate::snapshot::config_mismatch;
use crate::snapshot::load_snapshot;

/// What recovery did, counted exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The snapshot's last included sequence, when a snapshot was
    /// supplied.
    pub snapshot_seq: Option<u64>,
    /// Frames skipped because the snapshot already covered them.
    pub frames_skipped: usize,
    /// Frames replayed into the book.
    pub frames_applied: usize,
    /// Trailing torn bytes discarded by the scan (section 9.5).
    pub discarded_trailing_bytes: usize,
    /// The highest sequence recovered, from the snapshot or the log.
    pub last_recovered_seq: u64,
}

/// A recovered book and its report. No `Debug` derive: the book
/// itself does not implement `Debug`.
pub struct Recovery {
    /// The rebuilt book, ready for new commands.
    pub book: Book,
    /// Counts of what recovery consumed and applied.
    pub report: RecoveryReport,
}

/// Recover from an in-memory snapshot state (already decoded and
/// validated) and a log held in memory.
///
/// # Errors
///
/// Returns [`WalError::SnapshotConfigMismatch`] when the snapshot's
/// configuration differs from `config`, [`WalError::Book`] when the
/// base book cannot be built or a replayed command is rejected, and
/// the scanning errors of [`scan_log`], including
/// [`WalError::WalSequenceGap`] at the snapshot seam.
pub fn recover(
    config: BookConfig,
    snapshot: Option<SnapshotState>,
    wal_bytes: &[u8],
) -> Result<Recovery, WalError> {
    let (mut book, base_seq, snapshot_seq) = if let Some(state) = snapshot {
        if let Some(err) = config_mismatch(&state.config, &config) {
            return Err(err);
        }
        let base_seq = state
            .next_seq
            .checked_sub(1)
            .ok_or(WalError::CorruptSnapshot {
                detail: SnapshotCorruption::NextSeqMismatch {
                    last_included_seq: 0,
                    next_seq: 0,
                },
            })?;
        let book = Book::from_snapshot_state(state)
            .map_err(|source| WalError::Book { seq: None, source })?;
        (book, base_seq, Some(base_seq))
    } else {
        let book = Book::new(config).map_err(|source| WalError::Book { seq: None, source })?;
        (book, 0, None)
    };
    let scan = scan_log(wal_bytes)?;
    // The scratch event buffer is discarded frame by frame; it is
    // sized for the worst case a single command can emit (section 7.9
    // of the architecture: two events per resting order touched, plus
    // the terminal event).
    let mut scratch: Vec<ExecutionEvent> = Vec::with_capacity(2 * (config.max_orders as usize) + 2);
    let mut frames_skipped = 0usize;
    let mut frames_applied = 0usize;
    let mut last_recovered_seq = base_seq;
    for frame in &scan.frames {
        if frame.seq <= base_seq {
            frames_skipped += 1;
            continue;
        }
        let expected = last_recovered_seq.wrapping_add(1);
        if frame.seq != expected {
            return Err(WalError::WalSequenceGap {
                expected,
                found: frame.seq,
                offset: frame.offset,
            });
        }
        {
            let mut events = EventBuffer::new(&mut scratch);
            frame
                .command
                .apply(&mut book, &mut events)
                .map_err(|source| WalError::Book {
                    seq: Some(frame.seq),
                    source,
                })?;
        }
        scratch.clear();
        frames_applied += 1;
        last_recovered_seq = frame.seq;
    }
    Ok(Recovery {
        book,
        report: RecoveryReport {
            snapshot_seq,
            frames_skipped,
            frames_applied,
            discarded_trailing_bytes: scan.discarded_trailing_bytes,
            last_recovered_seq,
        },
    })
}

/// Recover from files: load the snapshot at `snapshot_path` when
/// given, read the log at `wal_path`, and recover.
///
/// # Errors
///
/// Returns the errors of [`load_snapshot`], [`crate::reader::read_log`],
/// and [`recover`].
pub fn recover_from_files(
    config: BookConfig,
    snapshot_path: Option<&Path>,
    wal_path: &Path,
) -> Result<Recovery, WalError> {
    let snapshot = match snapshot_path {
        Some(path) => Some(load_snapshot(path, &config)?),
        None => None,
    };
    let wal_bytes = std::fs::read(wal_path).map_err(|e| WalError::io("read WAL file", e))?;
    recover(config, snapshot, &wal_bytes)
}

#[cfg(test)]
mod tests {
    use chronicle_core::{OrderId, OrderType, ParticipantId, Side};

    use super::*;
    use crate::frame::{WalCommand, encode_frame};

    fn test_config() -> BookConfig {
        BookConfig {
            max_orders: 16,
            grid_len: 640,
            tick_size: 1,
            grid_start_tick: 1,
            default_stp: chronicle_core::StpPolicy::CancelIncoming,
        }
    }

    fn add(id: u64) -> WalCommand {
        WalCommand::Add {
            order_id: OrderId(id),
            participant: ParticipantId(1),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 1,
        }
    }

    fn log_of(frames: &[(WalCommand, u64)]) -> Vec<u8> {
        let mut log = Vec::new();
        let mut frame = Vec::new();
        for (command, seq) in frames {
            encode_frame(command, *seq, &mut frame);
            log.extend_from_slice(&frame);
        }
        log
    }

    /// Run the first `count` commands of a fixed script on a live
    /// book and return its captured state.
    fn state_after(count: usize) -> SnapshotState {
        let script = [
            add(1),
            add(2),
            WalCommand::Cancel {
                order_id: OrderId(1),
            },
            add(3),
            WalCommand::Amend {
                order_id: OrderId(2),
                new_price: 99,
                new_qty: 2,
            },
        ];
        let mut book = Book::new(test_config()).expect("book");
        let mut storage = Vec::with_capacity(256);
        for command in &script[..count] {
            {
                let mut events = chronicle_core::EventBuffer::new(&mut storage);
                command.apply(&mut book, &mut events).expect("apply");
            }
            storage.clear();
        }
        book.capture_snapshot_state()
    }

    #[test]
    fn wal_replay_skips_snapshot_covered_frames() {
        // INV-WAL-4: with a snapshot through seq 3, frames 1 to 3 are
        // skipped and 4 and 5 replay.
        let snapshot = state_after(3);
        assert_eq!(snapshot.next_seq, 4);
        let log = log_of(&[
            (add(1), 1),
            (add(2), 2),
            (
                WalCommand::Cancel {
                    order_id: OrderId(1),
                },
                3,
            ),
            (add(3), 4),
            (
                WalCommand::Amend {
                    order_id: OrderId(2),
                    new_price: 99,
                    new_qty: 2,
                },
                5,
            ),
        ]);
        let recovery = recover(test_config(), Some(snapshot), &log).expect("recover");
        assert_eq!(recovery.report.snapshot_seq, Some(3));
        assert_eq!(recovery.report.frames_skipped, 3);
        assert_eq!(recovery.report.frames_applied, 2);
        assert_eq!(recovery.report.last_recovered_seq, 5);
        assert_eq!(recovery.book.capture_snapshot_state(), state_after(5));
    }

    #[test]
    fn recover_seam_gap_is_an_error() {
        // INV-WAL-4: the first replayed frame after a snapshot must
        // be exactly last_included_seq + 1.
        let snapshot = state_after(3);
        let log = log_of(&[(add(3), 5)]);
        let err = recover(test_config(), Some(snapshot), &log)
            .err()
            .expect("seam gap");
        match err {
            WalError::WalSequenceGap {
                expected: 4,
                found: 5,
                offset: 0,
            } => {}
            other => panic!("expected seam gap (4, 5) at 0, got {other:?}"),
        }
    }

    #[test]
    fn recover_next_seq_is_max_plus_one() {
        // INV-SNAP-4: next_seq after recovery is the highest
        // recovered sequence plus one, counting the snapshot when
        // the log adds nothing.
        let log = log_of(&[(add(1), 1), (add(2), 2), (add(3), 3)]);
        let recovery = recover(test_config(), None, &log).expect("recover");
        assert_eq!(recovery.book.next_seq(), 4);
        assert_eq!(recovery.report.last_recovered_seq, 3);

        let snapshot = state_after(3);
        let recovery = recover(test_config(), Some(snapshot), &[]).expect("recover");
        assert_eq!(recovery.book.next_seq(), 4);
        assert_eq!(recovery.report.last_recovered_seq, 3);
        assert_eq!(recovery.report.frames_applied, 0);

        let recovery = recover(test_config(), None, &[]).expect("recover empty");
        assert_eq!(recovery.book.next_seq(), 1);
        assert_eq!(recovery.report.last_recovered_seq, 0);
    }

    #[test]
    fn recover_rejected_replay_is_a_typed_error() {
        // A log the book cannot replay (here, a duplicate order id
        // the writer side would never have accepted) fails with the
        // book's error at the frame's sequence, never a panic.
        let log = log_of(&[(add(1), 1), (add(1), 2)]);
        let err = recover(test_config(), None, &log)
            .err()
            .expect("duplicate id");
        match err {
            WalError::Book {
                seq: Some(2),
                source: chronicle_core::BookError::DuplicateOrderId { .. },
            } => {}
            other => panic!("expected a book error at seq 2, got {other:?}"),
        }
    }
}
