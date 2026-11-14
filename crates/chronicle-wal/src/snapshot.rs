//! Snapshot encoding, decoding, and the atomic save protocol
//! (ARCHITECTURE.md sections 10.2 to 10.4). A snapshot is a
//! structural dump of the book: header, generations, free list links,
//! level arrays, and live slot records, closed by a domain separated
//! checksum trailer. Saving follows the five step protocol of
//! section 10.3, so a crash at any point leaves either the old
//! snapshot or the new one whole, and a leftover `.tmp` file is never
//! loaded. Decoding validates in the order of section 10.4, with the
//! checksum last, and no partial state escapes a failed load.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chronicle_core::{BookConfig, SnapshotState};

use crate::crc::snapshot_crc;
use crate::error::WalError;
use crate::frame::stp_to_byte;

/// Snapshot magic bytes, ASCII `CSNP`.
pub const MAGIC: [u8; 4] = *b"CSNP";
/// The only snapshot format version.
pub const VERSION: u16 = 1;
/// Snapshot header length in bytes.
pub const HEADER_LEN: usize = 64;
/// Checksum trailer length in bytes.
pub const TRAILER_LEN: usize = 4;

/// One step of the atomic save protocol (section 10.3), reported to
/// the save hook in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStep {
    /// The snapshot is encoded into the save buffer.
    Encoded,
    /// The `.tmp` file is fully written.
    TmpWritten,
    /// The `.tmp` file is synced to durable storage.
    TmpSynced,
    /// The `.tmp` file is renamed over the target.
    Renamed,
    /// The parent directory is synced, making the rename durable.
    DirSynced,
}

/// The exact encoded length of a snapshot of `state`:
/// 68 + 8 per slot + 20 per level + 56 per live order (section
/// 10.2).
#[must_use]
pub fn encoded_len(state: &SnapshotState) -> usize {
    68 + 8 * state.generations.len() + 20 * state.levels.len() + 56 * state.live_orders.len()
}

/// Encode `state` into `out`, clearing any previous content. The
/// buffer is reused across calls, like the WAL writer's.
pub fn encode_snapshot(state: &SnapshotState, out: &mut Vec<u8>) {
    let total = encoded_len(state);
    out.clear();
    out.resize(total, 0);
    out[0..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&64u16.to_le_bytes());
    let last_included_seq = state
        .next_seq
        .checked_sub(1)
        .expect("a captured next_seq is at least 1");
    out[8..16].copy_from_slice(&last_included_seq.to_le_bytes());
    out[16..24].copy_from_slice(&state.next_seq.to_le_bytes());
    let live_count = u32::try_from(state.live_orders.len()).unwrap_or(0);
    out[24..28].copy_from_slice(&live_count.to_le_bytes());
    out[28..32].copy_from_slice(&state.config.max_orders.to_le_bytes());
    out[32..40].copy_from_slice(&state.config.tick_size.to_le_bytes());
    out[40..48].copy_from_slice(&state.config.grid_start_tick.to_le_bytes());
    out[48..52].copy_from_slice(&state.config.grid_len.to_le_bytes());
    out[52..56].copy_from_slice(&u32::from(stp_to_byte(state.config.default_stp)).to_le_bytes());
    out[56..60].copy_from_slice(&state.free_list_head.to_le_bytes());
    // Bytes 60..64 (reserved) stay zero from the resize.
    let mut pos = HEADER_LEN;
    for generation in &state.generations {
        out[pos..pos + 4].copy_from_slice(&generation.to_le_bytes());
        pos += 4;
    }
    for link in &state.free_links {
        out[pos..pos + 4].copy_from_slice(&link.to_le_bytes());
        pos += 4;
    }
    for level in &state.levels {
        out[pos..pos + 4].copy_from_slice(&level.head.to_le_bytes());
        out[pos + 4..pos + 8].copy_from_slice(&level.tail.to_le_bytes());
        out[pos + 8..pos + 16].copy_from_slice(&level.qty.to_le_bytes());
        out[pos + 16..pos + 20].copy_from_slice(&level.count.to_le_bytes());
        pos += 20;
    }
    for order in &state.live_orders {
        out[pos..pos + 4].copy_from_slice(&order.slot.to_le_bytes());
        out[pos + 4..pos + 8].copy_from_slice(&order.generation.to_le_bytes());
        out[pos + 8..pos + 16].copy_from_slice(&order.order_id.0.to_le_bytes());
        out[pos + 16..pos + 20].copy_from_slice(&order.participant.0.to_le_bytes());
        out[pos + 20..pos + 24].copy_from_slice(&order.price_idx.to_le_bytes());
        out[pos + 24..pos + 32].copy_from_slice(&order.qty.to_le_bytes());
        out[pos + 32] = crate::frame::side_to_byte(order.side);
        out[pos + 33] = 1; // live state byte
        // Bytes pos + 34 .. pos + 40 (padding) and pos + 48 ..
        // pos + 56 (reserved) stay zero from the resize.
        out[pos + 40..pos + 44].copy_from_slice(&order.prev.to_le_bytes());
        out[pos + 44..pos + 48].copy_from_slice(&order.next.to_le_bytes());
        pos += 56;
    }
    debug_assert_eq!(pos, total - TRAILER_LEN);
    let crc = snapshot_crc(&out[..total - TRAILER_LEN]);
    out[total - TRAILER_LEN..total].copy_from_slice(&crc.to_le_bytes());
}

/// Decode and fully validate a snapshot held in memory, following
/// the validation order of section 10.4: file size against the
/// header arithmetic, magic, version, header length, sequence
/// consistency, configuration compatibility with `expected`, the
/// structural checks (free list exactness, live record consistency),
/// and finally the trailing checksum.
///
/// # Errors
///
/// Returns [`WalError::CorruptSnapshot`] for any structural or
/// checksum failure and [`WalError::SnapshotConfigMismatch`] when a
/// configuration field differs from `expected`.
pub fn decode_snapshot(bytes: &[u8], expected: &BookConfig) -> Result<SnapshotState, WalError> {
    let _ = (bytes, expected);
    panic!("M3 Stage A stub: snapshot decoding lands in Stage B")
}

/// The `.tmp` sibling path used by the save protocol.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Save a snapshot atomically (the protocol of section 10.3): encode,
/// write the `.tmp` sibling, sync it, rename it over the target,
/// then sync the parent directory.
///
/// # Errors
///
/// Returns [`WalError::Io`] when any step fails.
pub fn save_snapshot(path: &Path, state: &SnapshotState) -> Result<(), WalError> {
    save_snapshot_with_hook(path, state, &mut |_| true)
}

/// [`save_snapshot`] with a hook called after each protocol step.
/// Returning `false` from the hook stops the save at that point,
/// exactly as a crash would: the function returns success with the
/// filesystem left mid-protocol. Recovery tests drive the crash
/// oracle through this hook (INV-SNAP-2); production callers use
/// [`save_snapshot`].
///
/// # Errors
///
/// Returns [`WalError::Io`] when any step fails.
pub fn save_snapshot_with_hook(
    path: &Path,
    state: &SnapshotState,
    hook: &mut dyn FnMut(SaveStep) -> bool,
) -> Result<(), WalError> {
    let mut buffer = Vec::new();
    encode_snapshot(state, &mut buffer);
    if !hook(SaveStep::Encoded) {
        return Ok(());
    }
    let tmp = tmp_path(path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| WalError::io("open snapshot temp file", e))?;
        file.write_all(&buffer)
            .map_err(|e| WalError::io("write snapshot temp file", e))?;
        if !hook(SaveStep::TmpWritten) {
            return Ok(());
        }
        file.sync_all()
            .map_err(|e| WalError::io("sync snapshot temp file", e))?;
    }
    if !hook(SaveStep::TmpSynced) {
        return Ok(());
    }
    std::fs::rename(&tmp, path).map_err(|e| WalError::io("rename snapshot into place", e))?;
    if !hook(SaveStep::Renamed) {
        return Ok(());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let dir = File::open(parent).map_err(|e| WalError::io("open snapshot directory", e))?;
    dir.sync_all()
        .map_err(|e| WalError::io("sync snapshot directory", e))?;
    hook(SaveStep::DirSynced);
    Ok(())
}

/// Load and validate the snapshot at `path` for a book configured by
/// `expected`. A leftover `.tmp` sibling is never consulted.
///
/// # Errors
///
/// Returns [`WalError::Io`] when the file cannot be read, and the
/// validation errors of [`decode_snapshot`].
pub fn load_snapshot(path: &Path, expected: &BookConfig) -> Result<SnapshotState, WalError> {
    let bytes = std::fs::read(path).map_err(|e| WalError::io("read snapshot file", e))?;
    decode_snapshot(&bytes, expected)
}

#[cfg(test)]
mod tests {
    use chronicle_core::{
        Book, EventBuffer, LevelState, LiveOrderState, OrderId, OrderType, ParticipantId, Side,
        StpPolicy,
    };

    use super::*;
    use crate::error::SnapshotCorruption;

    fn test_config() -> BookConfig {
        BookConfig {
            max_orders: 16,
            grid_len: 640,
            tick_size: 1,
            grid_start_tick: 1,
            default_stp: StpPolicy::CancelIncoming,
        }
    }

    /// A unique scratch directory under the system temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chronicle-wal-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn hand_state() -> SnapshotState {
        let mut levels = vec![
            LevelState {
                head: u32::MAX,
                tail: u32::MAX,
                qty: 0,
                count: 0,
            };
            64
        ];
        levels[5] = LevelState {
            head: 0,
            tail: 0,
            qty: 9,
            count: 1,
        };
        SnapshotState {
            config: BookConfig {
                max_orders: 2,
                grid_len: 64,
                tick_size: 1,
                grid_start_tick: 1,
                default_stp: StpPolicy::CancelResting,
            },
            next_seq: 7,
            generations: vec![3, 1],
            free_list_head: 1,
            free_links: vec![u32::MAX, u32::MAX],
            levels,
            live_orders: vec![LiveOrderState {
                slot: 0,
                generation: 3,
                order_id: OrderId(11),
                participant: ParticipantId(4),
                price_idx: 5,
                qty: 9,
                side: Side::Bid,
                prev: u32::MAX,
                next: u32::MAX,
            }],
        }
    }

    #[test]
    fn snapshot_layout_golden() {
        // INV-SNAP-1: header fields, body sections, and the trailer
        // land at the exact offsets of section 10.2, and the total
        // size matches the section 10.2 formula.
        let state = hand_state();
        let mut out = Vec::new();
        encode_snapshot(&state, &mut out);
        assert_eq!(out.len(), 68 + 8 * 2 + 20 * 64 + 56);
        assert_eq!(&out[0..4], b"CSNP");
        assert_eq!(&out[4..6], &1u16.to_le_bytes());
        assert_eq!(&out[6..8], &64u16.to_le_bytes());
        assert_eq!(&out[8..16], &6u64.to_le_bytes(), "last_included_seq");
        assert_eq!(&out[16..24], &7u64.to_le_bytes(), "next_seq");
        assert_eq!(&out[24..28], &1u32.to_le_bytes(), "live_order_count");
        assert_eq!(&out[28..32], &2u32.to_le_bytes(), "max_orders");
        assert_eq!(
            &out[52..56],
            &1u32.to_le_bytes(),
            "default_stp cancel resting"
        );
        assert_eq!(&out[56..60], &1u32.to_le_bytes(), "free_list_head");
        assert_eq!(&out[60..64], &[0u8; 4], "reserved");
        // Body: generations, then free links.
        assert_eq!(&out[64..68], &3u32.to_le_bytes());
        assert_eq!(&out[68..72], &1u32.to_le_bytes());
        assert_eq!(&out[72..80], &[0xFFu8; 8], "both links NIL");
        // Level 5 entry starts at 80 + 5 * 20 = 180.
        assert_eq!(&out[180..184], &0u32.to_le_bytes(), "level head");
        assert_eq!(&out[188..196], &9u64.to_le_bytes(), "level qty");
        assert_eq!(&out[196..200], &1u32.to_le_bytes(), "level count");
        // The live record starts at 80 + 64 * 20 = 1360.
        assert_eq!(&out[1360..1364], &0u32.to_le_bytes(), "record slot");
        assert_eq!(&out[1364..1368], &3u32.to_le_bytes(), "record generation");
        assert_eq!(&out[1368..1376], &11u64.to_le_bytes(), "record order id");
        assert_eq!(out[1392], 0, "record side bid");
        assert_eq!(out[1393], 1, "record state live");
        let stored = u32::from_le_bytes([
            out[out.len() - 4],
            out[out.len() - 3],
            out[out.len() - 2],
            out[out.len() - 1],
        ]);
        assert_eq!(stored, snapshot_crc(&out[..out.len() - 4]));
    }

    #[test]
    fn snapshot_encode_decode_roundtrip() {
        let state = hand_state();
        let mut out = Vec::new();
        encode_snapshot(&state, &mut out);
        let decoded = decode_snapshot(&out, &state.config).expect("a written snapshot decodes");
        assert_eq!(decoded, state);
    }

    #[test]
    fn tamper_snapshot_every_header_field() {
        // INV-SNAP-1: flipping any single header byte is a typed
        // error, never a decode and never a panic.
        let state = hand_state();
        let mut out = Vec::new();
        encode_snapshot(&state, &mut out);
        for pos in 0..64 {
            let mut damaged = out.clone();
            damaged[pos] = damaged[pos].wrapping_add(1);
            let err = decode_snapshot(&damaged, &state.config)
                .expect_err(&format!("header byte {pos} flipped"));
            assert!(
                matches!(
                    err,
                    WalError::CorruptSnapshot { .. } | WalError::SnapshotConfigMismatch { .. }
                ),
                "header byte {pos} gave {err:?}"
            );
        }
    }

    #[test]
    fn tamper_snapshot_body_and_trailer() {
        // INV-SNAP-1: body and trailer damage is caught, by the
        // structural checks or by the trailing checksum.
        let state = hand_state();
        let mut out = Vec::new();
        encode_snapshot(&state, &mut out);
        for pos in [64usize, 100, 500, 1365, 1400] {
            let mut damaged = out.clone();
            damaged[pos] = damaged[pos].wrapping_add(1);
            assert!(
                decode_snapshot(&damaged, &state.config).is_err(),
                "body byte {pos} flipped must fail"
            );
        }
        let mut damaged = out.clone();
        let last = damaged.len() - 1;
        damaged[last] = damaged[last].wrapping_add(1);
        let err = decode_snapshot(&damaged, &state.config).expect_err("trailer damaged");
        match err {
            WalError::CorruptSnapshot {
                detail: SnapshotCorruption::CrcMismatch { .. },
            } => {}
            other => panic!("expected a CRC mismatch, got {other:?}"),
        }
    }

    #[test]
    fn fault_snapshot_crash_at_each_step() {
        // INV-SNAP-2: interrupting the save protocol between any two
        // steps leaves a loadable snapshot: the old one before the
        // rename, the new one after it, always whole.
        let dir = scratch("crash-steps");
        let path = dir.join("book.csnp");
        let config = test_config();
        let old_state = Book::new(config).expect("book").capture_snapshot_state();
        save_snapshot(&path, &old_state).expect("initial save");
        let mut new_book = Book::new(config).expect("book");
        {
            let mut storage = Vec::with_capacity(16);
            let mut events = EventBuffer::new(&mut storage);
            new_book
                .add_order(
                    OrderId(1),
                    ParticipantId(1),
                    Side::Bid,
                    OrderType::Limit,
                    100,
                    5,
                    None,
                    &mut events,
                )
                .expect("add");
        }
        let new_state = new_book.capture_snapshot_state();
        for stop in [
            SaveStep::Encoded,
            SaveStep::TmpWritten,
            SaveStep::TmpSynced,
            SaveStep::Renamed,
            SaveStep::DirSynced,
        ] {
            save_snapshot(&path, &old_state).expect("reset to the old snapshot");
            save_snapshot_with_hook(&path, &new_state, &mut |step| step != stop)
                .expect("interrupted save is not an error");
            let loaded = load_snapshot(&path, &config).expect("load after interruption");
            let expected = if matches!(stop, SaveStep::Renamed | SaveStep::DirSynced) {
                &new_state
            } else {
                &old_state
            };
            assert_eq!(&loaded, expected, "interrupted after {stop:?}");
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn snapshot_loader_ignores_tmp_file() {
        // INV-SNAP-2: a leftover .tmp is never loaded, however valid
        // or invalid its content.
        let dir = scratch("tmp-ignore");
        let path = dir.join("book.csnp");
        let config = test_config();
        let state = Book::new(config).expect("book").capture_snapshot_state();
        save_snapshot(&path, &state).expect("save");
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        std::fs::write(&tmp_name, b"garbage, not a snapshot").expect("plant tmp file");
        let loaded = load_snapshot(&path, &config).expect("the real snapshot loads");
        assert_eq!(loaded, state);
        // With no target at all, a lone .tmp does not become a load.
        std::fs::remove_file(&path).expect("remove target");
        let err = load_snapshot(&path, &config).expect_err("no target, only a tmp");
        assert!(matches!(err, WalError::Io { .. }), "got {err:?}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn snapshot_config_mismatch_names_field() {
        // INV-SNAP-5: each configuration field, varied in turn, is
        // named by the mismatch error together with both values.
        let state = hand_state();
        let mut out = Vec::new();
        encode_snapshot(&state, &mut out);
        let cases: [(&str, BookConfig); 5] = [
            (
                "max_orders",
                BookConfig {
                    max_orders: 4,
                    ..state.config
                },
            ),
            (
                "tick_size",
                BookConfig {
                    tick_size: 2,
                    ..state.config
                },
            ),
            (
                "grid_start_tick",
                BookConfig {
                    grid_start_tick: 9,
                    ..state.config
                },
            ),
            (
                "grid_len",
                BookConfig {
                    grid_len: 128,
                    ..state.config
                },
            ),
            (
                "default_stp",
                BookConfig {
                    default_stp: StpPolicy::CancelBoth,
                    ..state.config
                },
            ),
        ];
        for (field, expected_config) in cases {
            let err =
                decode_snapshot(&out, &expected_config).expect_err(&format!("{field} differs"));
            match err {
                WalError::SnapshotConfigMismatch { field: named, .. } => {
                    assert_eq!(named, field);
                }
                other => panic!("expected a config mismatch on {field}, got {other:?}"),
            }
        }
    }

    #[test]
    fn snapshot_restore_preserves_generations() {
        // INV-SNAP-4: generations survive the round trip exactly, so
        // a stale pre-snapshot handle stays rejected and the restored
        // book allocates and emits exactly like the original.
        let config = test_config();
        let mut book = Book::new(config).expect("book");
        let mut run = |book: &mut Book,
                       f: &dyn Fn(
            &mut Book,
            &mut EventBuffer<'_>,
        ) -> Result<usize, chronicle_core::BookError>| {
            let mut storage = Vec::with_capacity(64);
            let mut events = EventBuffer::new(&mut storage);
            f(book, &mut events).expect("command");
        };
        run(&mut book, &|b, e| {
            b.add_order(
                OrderId(1),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                5,
                None,
                e,
            )
        });
        run(&mut book, &|b, e| b.cancel_order(OrderId(1), e));
        run(&mut book, &|b, e| {
            b.add_order(
                OrderId(2),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                5,
                None,
                e,
            )
        });
        let state = book.capture_snapshot_state();
        assert_eq!(state.live_orders[0].generation, 2, "slot 0 was reused once");
        let mut bytes = Vec::new();
        encode_snapshot(&state, &mut bytes);
        let decoded = decode_snapshot(&bytes, &config).expect("decode");
        let mut restored = Book::from_snapshot_state(decoded).expect("restore");
        assert_eq!(restored.capture_snapshot_state(), state);
        // Both books take the same next commands identically.
        let script = |book: &mut Book| -> SnapshotState {
            let mut storage = Vec::with_capacity(64);
            {
                let mut events = EventBuffer::new(&mut storage);
                book.add_order(
                    OrderId(3),
                    ParticipantId(2),
                    Side::Ask,
                    OrderType::Limit,
                    101,
                    5,
                    None,
                    &mut events,
                )
                .expect("add");
            }
            book.capture_snapshot_state()
        };
        assert_eq!(script(&mut restored), script(&mut book));
    }
}
