//! The WAL reader (ARCHITECTURE.md section 9.5). Scanning walks
//! frames from the front of the log, applying the section 9.5
//! decision table at each position through [`crate::frame::parse_frame`],
//! and enforces sequence consecutiveness within the stream: the
//! first frame sets the base, and every later frame must carry
//! exactly the previous sequence plus one. Recovery adds the
//! snapshot seam check on top of this (section 10.4).
//!
//! The scan comes in three forms over the one decision table: a
//! whole-slice scan ([`scan_log`]), a streaming scanner fed in
//! chunks ([`LogScanner`]), and a reader-driven scan ([`scan_reader`])
//! for logs too large to hold in memory (programme 2, lane S5).

use std::io::Read;
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
    let mut frames = Vec::new();
    let mut previous_seq: Option<u64> = None;
    let mut pos = 0usize;
    while pos < bytes.len() {
        let offset = u64::try_from(pos).unwrap_or(u64::MAX);
        match crate::frame::parse_frame(&bytes[pos..], offset)? {
            crate::frame::FrameParse::Frame {
                seq,
                command,
                frame_len,
            } => {
                if let Some(previous) = previous_seq
                    && previous.checked_add(1) != Some(seq)
                {
                    return Err(WalError::WalSequenceGap {
                        expected: previous.wrapping_add(1),
                        found: seq,
                        offset,
                    });
                }
                previous_seq = Some(seq);
                frames.push(ScannedFrame {
                    offset,
                    seq,
                    command,
                });
                pos += frame_len;
            }
            crate::frame::FrameParse::CrashBoundary { discarded } => {
                return Ok(ScanOutcome {
                    frames,
                    discarded_trailing_bytes: discarded,
                });
            }
        }
    }
    Ok(ScanOutcome {
        frames,
        discarded_trailing_bytes: 0,
    })
}

/// A streaming log scanner: the section 9.5 decision table applied
/// incrementally to chunks of a log, for readers that do not hold
/// the whole log in memory.
///
/// Feed chunks in order with [`LogScanner::feed`], which appends
/// every frame the chunk completes to the caller's vector, then
/// close the stream with [`LogScanner::finish`], which resolves
/// the held tail and reports the discarded trailing byte count.
/// The outcome is identical to [`scan_log`] over the concatenated
/// bytes. The scanner holds back only the bytes of one undecided
/// tail, a partial frame or a header fragment, because the crash
/// boundary verdicts of section 9.5 are decidable only at the true
/// end of the log. Corruption and sequence gap errors are decidable
/// the moment their bytes arrive and surface from `feed` directly.
#[derive(Clone, Debug, Default)]
pub struct LogScanner {
    pending: Vec<u8>,
    base_offset: u64,
    previous_seq: Option<u64>,
}

impl LogScanner {
    /// A fresh scanner at the start of a log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk of the log, appending every frame it
    /// completes to `frames`.
    ///
    /// # Errors
    ///
    /// Returns the scanning errors of [`scan_log`] for the bytes
    /// seen so far: [`WalError::CorruptWalRecord`] at the first
    /// corrupt frame and [`WalError::WalSequenceGap`] at the first
    /// break in sequence consecutiveness.
    pub fn feed(&mut self, chunk: &[u8], frames: &mut Vec<ScannedFrame>) -> Result<(), WalError> {
        self.pending.extend_from_slice(chunk);
        self.drain(frames, false)?;
        Ok(())
    }

    /// Close the stream, resolving the held tail as the end of the
    /// log. Appends any frame the tail completes to `frames` and
    /// returns the discarded trailing byte count, which is zero for
    /// a log that ends exactly on a frame boundary.
    ///
    /// # Errors
    ///
    /// Returns the scanning errors of [`scan_log`] for the held
    /// tail.
    pub fn finish(mut self, frames: &mut Vec<ScannedFrame>) -> Result<usize, WalError> {
        self.drain(frames, true)
    }

    /// Walk the pending bytes, consuming every decidable frame into
    /// `frames`. At the true end of the log (`end_of_log`), a crash
    /// boundary verdict resolves the tail and its byte count is
    /// returned; mid-stream it only pauses the walk.
    fn drain(
        &mut self,
        frames: &mut Vec<ScannedFrame>,
        end_of_log: bool,
    ) -> Result<usize, WalError> {
        let mut consumed = 0usize;
        let mut discarded = 0usize;
        loop {
            let slice = &self.pending[consumed..];
            if slice.is_empty() {
                break;
            }
            let offset = self.base_offset + u64::try_from(consumed).unwrap_or(u64::MAX);
            match crate::frame::parse_frame(slice, offset)? {
                crate::frame::FrameParse::Frame {
                    seq,
                    command,
                    frame_len,
                } => {
                    if let Some(previous) = self.previous_seq
                        && previous.checked_add(1) != Some(seq)
                    {
                        return Err(WalError::WalSequenceGap {
                            expected: previous.wrapping_add(1),
                            found: seq,
                            offset,
                        });
                    }
                    self.previous_seq = Some(seq);
                    frames.push(ScannedFrame {
                        offset,
                        seq,
                        command,
                    });
                    consumed += frame_len;
                }
                crate::frame::FrameParse::CrashBoundary { discarded: tail } => {
                    if end_of_log {
                        discarded = tail;
                        consumed += tail;
                    }
                    break;
                }
            }
        }
        self.pending.drain(..consumed);
        self.base_offset += u64::try_from(consumed).unwrap_or(u64::MAX);
        Ok(discarded)
    }
}

/// Scan a log from any byte reader, in bounded memory: the input
/// is consumed in fixed-size chunks through a [`LogScanner`], so
/// only the decoded frames accumulate.
///
/// # Errors
///
/// Returns [`WalError::Io`] when the reader fails, and the scanning
/// errors of [`scan_log`].
pub fn scan_reader(mut reader: impl Read) -> Result<ScanOutcome, WalError> {
    let mut scanner = LogScanner::new();
    let mut frames = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|e| WalError::io("read WAL stream", e))?;
        if read == 0 {
            break;
        }
        scanner.feed(&chunk[..read], &mut frames)?;
    }
    let discarded_trailing_bytes = scanner.finish(&mut frames)?;
    Ok(ScanOutcome {
        frames,
        discarded_trailing_bytes,
    })
}

/// Read a log file and scan it, streaming the file through
/// [`scan_reader`] rather than holding it whole in memory.
///
/// # Errors
///
/// Returns [`WalError::Io`] when the file cannot be read, and the
/// scanning errors of [`scan_log`].
pub fn read_log(path: &Path) -> Result<ScanOutcome, WalError> {
    let file = std::fs::File::open(path).map_err(|e| WalError::io("read WAL file", e))?;
    scan_reader(file)
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
                .rev()
                .find(|&&end| end <= cut)
                .map_or(0, |&end| end);
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
        let mut damaged = log;
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
                let at = first.len() + pos;
                // The kind byte is fixed as a set, not a value, so
                // flip it clear of every known kind; each other
                // fixed byte holds exactly one value, and adding
                // one breaks it.
                damaged[at] = if pos == 20 {
                    0xEE
                } else {
                    damaged[at].wrapping_add(1)
                };
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
        assert_eq!(outcome.frames.len(), 0);
        assert_eq!(outcome.discarded_trailing_bytes, 0);
    }

    /// Drive a streaming scanner over `bytes` in chunks of
    /// `chunk_len` and collect the outcome `scan_log` would shape.
    fn streamed(bytes: &[u8], chunk_len: usize) -> Result<ScanOutcome, WalError> {
        let mut scanner = LogScanner::new();
        let mut frames = Vec::new();
        for chunk in bytes.chunks(chunk_len.max(1)) {
            scanner.feed(chunk, &mut frames)?;
        }
        let discarded_trailing_bytes = scanner.finish(&mut frames)?;
        Ok(ScanOutcome {
            frames,
            discarded_trailing_bytes,
        })
    }

    #[test]
    fn streaming_scan_matches_scan_log_at_every_cut_and_chunking() {
        // Lane S5: the streaming scanner is the whole-slice scan
        // under chunking. Every truncation of the fixture log (so
        // every torn shape the section 9.5 table decides) streams
        // to exactly the scan_log outcome at chunk sizes that split
        // headers, payloads, and frames at different phases.
        let log = three_frame_log();
        for cut in 0..=log.len() {
            let expected = scan_log(&log[..cut]).expect("the slice scan is clean");
            for chunk_len in [1_usize, 7, 31, 32, 33, 72, 184] {
                let outcome = streamed(&log[..cut], chunk_len).unwrap_or_else(|e| {
                    panic!("cut {cut} chunk {chunk_len} must scan clean, got {e}")
                });
                assert_eq!(outcome, expected, "cut {cut} chunk {chunk_len}");
            }
            let via_reader =
                scan_reader(std::io::Cursor::new(&log[..cut])).expect("the reader scan is clean");
            assert_eq!(via_reader, expected, "reader at cut {cut}");
        }
    }

    #[test]
    fn streaming_scan_surfaces_the_same_errors() {
        // Corruption and sequence gaps surface from the streaming
        // scanner with the same values the slice scan reports, at
        // the feed that delivers the deciding bytes.
        let mut damaged = log_of(&[(add(1), 1), (add(2), 2)]);
        damaged[28] = damaged[28].wrapping_add(1);
        let whole_err = scan_log(&damaged).expect_err("the slice scan convicts the checksum");
        let mut scanner = LogScanner::new();
        let mut frames = Vec::new();
        let stream_err = scanner
            .feed(&damaged, &mut frames)
            .expect_err("the streaming scan convicts the checksum");
        assert_eq!(stream_err.to_string(), whole_err.to_string());

        let gap_log = log_of(&[(add(1), 1), (add(2), 3)]);
        let whole_err = scan_log(&gap_log).expect_err("the slice scan convicts the gap");
        let mut scanner = LogScanner::new();
        let mut frames = Vec::new();
        scanner
            .feed(&gap_log[..72], &mut frames)
            .expect("the first frame is clean");
        let stream_err = scanner
            .feed(&gap_log[72..], &mut frames)
            .expect_err("the streaming scan convicts the gap");
        assert_eq!(stream_err.to_string(), whole_err.to_string());
    }
}
