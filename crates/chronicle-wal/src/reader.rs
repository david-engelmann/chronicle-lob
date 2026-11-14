//! The WAL reader (ARCHITECTURE.md section 9.5). Scanning walks
//! frames from the front of the log, applying the section 9.5
//! decision table at each position through [`crate::frame::parse_frame`],
//! and enforces sequence consecutiveness within the stream: the
//! first frame sets the base, and every later frame must carry
//! exactly the previous sequence plus one. Recovery adds the
//! snapshot seam check on top of this (section 10.4).

use std::path::Path;

use crate::error::WalError;
use crate::frame::WalCommand;

/// One decoded frame and where it sat in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScannedFrame {
    /// Byte offset of the frame's first byte.
    pub offset: u64,
    /// The frame's authoritative sequence number.
    pub seq: u64,
    /// The decoded command.
    pub command: WalCommand,
}

/// The result of scanning a whole log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOutcome {
    /// Every complete, valid frame, in log order.
    pub frames: Vec<ScannedFrame>,
    /// Trailing bytes discarded as a torn crash tail (section 9.5).
    /// Zero for a log that ends exactly on a frame boundary.
    pub discarded_trailing_bytes: usize,
}

/// Scan a whole log held in memory.
///
/// # Errors
///
/// Returns [`WalError::CorruptWalRecord`] at the first corrupt frame
/// and [`WalError::WalSequenceGap`] at the first break in sequence
/// consecutiveness. A torn tail is not an error: it is reported in
/// [`ScanOutcome::discarded_trailing_bytes`].
pub fn scan_log(bytes: &[u8]) -> Result<ScanOutcome, WalError> {
    let _ = bytes;
    panic!("M3 Stage A stub: log scanning lands in Stage B")
}

/// Read a log file and scan it.
///
/// # Errors
///
/// Returns [`WalError::Io`] when the file cannot be read, and the
/// scanning errors of [`scan_log`].
pub fn read_log(path: &Path) -> Result<ScanOutcome, WalError> {
    let bytes = std::fs::read(path).map_err(|e| WalError::io("read WAL file", e))?;
    scan_log(&bytes)
}

#[cfg(test)]
mod tests {
    use chronicle_core::{OrderId, OrderType, ParticipantId, Side};

    use super::*;
    use crate::error::WalCorruption;
    use crate::frame::encode_frame;

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

    /// Encode (command, seq) pairs into one log byte string.
    fn log_of(frames: &[(WalCommand, u64)]) -> Vec<u8> {
        let mut log = Vec::new();
        let mut frame = Vec::new();
        for (command, seq) in frames {
            encode_frame(command, *seq, &mut frame);
            log.extend_from_slice(&frame);
        }
        log
    }

    fn three_frame_log() -> Vec<u8> {
        log_of(&[
            (add(1), 1),
            (
                WalCommand::Cancel {
                    order_id: OrderId(1),
                },
                2,
            ),
            (
                WalCommand::Amend {
                    order_id: OrderId(2),
                    new_price: 99,
                    new_qty: 2,
                },
                3,
            ),
        ])
    }

    #[test]
    fn fault_wal_truncate_at_every_offset() {
        // INV-WAL-2: truncating a valid log at any byte yields the
        // complete frames before the cut and a reported crash tail,
        // never corruption and never a partial frame.
        let log = three_frame_log();
        let frame_ends = [72usize, 120, 184];
        assert_eq!(log.len(), 184);
        for cut in 0..=log.len() {
            let outcome = scan_log(&log[..cut])
                .unwrap_or_else(|e| panic!("cut {cut} must scan clean, got {e}"));
            let complete = frame_ends.iter().filter(|&&end| end <= cut).count();
            let last_end = frame_ends
                .iter()
                .filter(|&&end| end <= cut)
                .last()
                .copied()
                .unwrap_or(0);
            assert_eq!(outcome.frames.len(), complete, "cut {cut}");
            assert_eq!(
                outcome.discarded_trailing_bytes,
                cut - last_end,
                "cut {cut}"
            );
            for (i, frame) in outcome.frames.iter().enumerate() {
                assert_eq!(frame.seq, (i + 1) as u64, "cut {cut}");
            }
        }
    }

    #[test]
    fn fault_wal_corrupt_bytes() {
        // INV-WAL-2 and INV-WAL-3: flipping any single byte of a
        // valid log is corruption at a frame offset, caught by the
        // field checks or the checksum, never accepted and never a
        // crash boundary.
        let log = log_of(&[
            (add(1), 1),
            (
                WalCommand::Cancel {
                    order_id: OrderId(1),
                },
                2,
            ),
        ]);
        assert_eq!(log.len(), 120);
        for pos in 0..log.len() {
            let mut damaged = log.clone();
            damaged[pos] = damaged[pos].wrapping_add(1);
            let err =
                scan_log(&damaged).expect_err(&format!("flip at byte {pos} must be corruption"));
            assert!(
                matches!(err, WalError::CorruptWalRecord { .. }),
                "flip at byte {pos} gave {err:?}"
            );
        }
        // A checksum flip names both checksums (INV-WAL-3).
        let mut damaged = log.clone();
        damaged[28] = damaged[28].wrapping_add(1);
        let err = scan_log(&damaged).expect_err("stored checksum damaged");
        match err {
            WalError::CorruptWalRecord {
                offset: 0,
                detail: WalCorruption::CrcMismatch { .. },
            } => {}
            other => panic!("expected a CRC mismatch at offset 0, got {other:?}"),
        }
    }

    #[test]
    fn fault_wal_fragment_prefix_rule() {
        // INV-WAL-2: a trailing fragment shorter than a header is a
        // clean crash boundary when every fixed-value byte present
        // matches a valid header prefix, and corruption at the
        // fragment start when any one does not, down to a wrong first
        // byte in a one byte fragment.
        let first = log_of(&[(add(1), 1)]);
        let mut second_frame = Vec::new();
        encode_frame(
            &WalCommand::Cancel {
                order_id: OrderId(1),
            },
            2,
            &mut second_frame,
        );
        // Byte positions inside a header whose values are fixed by
        // the format: magic 0..4, version 4..6, header length 6..8,
        // kind 20, flags 21, reserved 22..28.
        let fixed_positions = [0usize, 1, 2, 3, 4, 5, 6, 7, 20, 21, 22, 23, 24, 25, 26, 27];
        for len in 1..32usize {
            let mut log = first.clone();
            log.extend_from_slice(&second_frame[..len]);
            let outcome = scan_log(&log).expect("prefix-valid fragment is a crash boundary");
            assert_eq!(outcome.frames.len(), 1, "fragment length {len}");
            assert_eq!(
                outcome.discarded_trailing_bytes, len,
                "fragment length {len}"
            );
            for &pos in fixed_positions.iter().filter(|&&p| p < len) {
                let mut damaged = log.clone();
                damaged[first.len() + pos] = damaged[first.len() + pos].wrapping_add(1);
                let err = scan_log(&damaged)
                    .expect_err(&format!("fragment {len} with byte {pos} flipped"));
                assert!(
                    matches!(err, WalError::CorruptWalRecord { offset: 72, .. }),
                    "fragment {len} byte {pos} gave {err:?}"
                );
            }
        }
    }

    #[test]
    fn fault_wal_sequence_gap() {
        // INV-WAL-4: a missing or duplicated sequence inside the
        // stream is a gap error naming the expected and found values
        // and the frame offset.
        let log = log_of(&[
            (add(1), 1),
            (
                WalCommand::Cancel {
                    order_id: OrderId(1),
                },
                2,
            ),
            (add(2), 4),
        ]);
        let err = scan_log(&log).expect_err("seq 3 is missing");
        match err {
            WalError::WalSequenceGap {
                expected: 3,
                found: 4,
                offset: 120,
            } => {}
            other => panic!("expected gap (3, 4) at 120, got {other:?}"),
        }
        let log = log_of(&[
            (add(1), 1),
            (
                WalCommand::Cancel {
                    order_id: OrderId(1),
                },
                2,
            ),
            (add(2), 2),
        ]);
        let err = scan_log(&log).expect_err("seq 2 repeats");
        match err {
            WalError::WalSequenceGap {
                expected: 3,
                found: 2,
                offset: 120,
            } => {}
            other => panic!("expected gap (3, 2) at 120, got {other:?}"),
        }
        // A stream may start at any sequence (it can continue a log
        // a snapshot already covered), and an empty log scans clean.
        let log = log_of(&[(add(1), 41), (add(2), 42)]);
        assert_eq!(scan_log(&log).expect("mid-stream start").frames.len(), 2);
        let outcome = scan_log(&[]).expect("empty log");
        assert!(outcome.frames.is_empty());
        assert_eq!(outcome.discarded_trailing_bytes, 0);
    }
}
