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

use chronicle_core::{
    BookConfig, LevelState, LiveOrderState, OrderId, ParticipantId, SnapshotState, StpPolicy,
};

use crate::crc::snapshot_crc;
use crate::error::{SnapshotCorruption, WalError};
use crate::frame::{side_from_byte, stp_to_byte};

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
pub const fn encoded_len(state: &SnapshotState) -> usize {
    68 + 8 * state.generations.len() + 20 * state.levels.len() + 56 * state.live_orders.len()
}

/// Encode `state` into `out`, clearing any previous content. The
/// buffer is reused across calls, like the WAL writer's.
///
/// # Panics
///
/// Panics when `state.next_seq` is zero. A state captured from a
/// book always has a next sequence of at least 1, because sequence
/// numbers start at 1.
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

/// Decode and fully validate a snapshot held in memory.
///
/// Validation follows the order of section 10.4: file size against
/// the header arithmetic, magic, version, header length, sequence
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
    let header = read_header(bytes)?;
    if let Some(err) = config_mismatch(&header.config, expected) {
        return Err(err);
    }
    let max_orders = header.config.max_orders;
    let grid_len = header.config.grid_len;
    // Body sections, in the fixed order of section 10.2.
    let slots = max_orders as usize;
    let mut pos = HEADER_LEN;
    let mut generations = Vec::with_capacity(slots);
    for _ in 0..max_orders {
        generations.push(read_u32(bytes, pos));
        pos += 4;
    }
    let mut free_links = Vec::with_capacity(slots);
    for _ in 0..max_orders {
        free_links.push(read_u32(bytes, pos));
        pos += 4;
    }
    let levels = decode_levels(bytes, &mut pos, grid_len, max_orders)?;
    let (live_orders, live_seen) =
        decode_live_records(bytes, &mut pos, &header, &generations, &free_links)?;
    validate_free_list(&header, &generations, &free_links, &live_seen)?;
    // The checksum is validated last (section 10.4).
    let actual_len = bytes.len();
    let stored_crc = read_u32(bytes, actual_len - TRAILER_LEN);
    let computed_crc = snapshot_crc(&bytes[..actual_len - TRAILER_LEN]);
    if computed_crc != stored_crc {
        return Err(corrupt(SnapshotCorruption::CrcMismatch {
            expected_crc: computed_crc,
            actual_crc: stored_crc,
        }));
    }
    Ok(SnapshotState {
        config: *expected,
        next_seq: header.next_seq,
        generations,
        free_list_head: header.free_list_head,
        free_links,
        levels,
        live_orders,
    })
}

/// A parsed snapshot header: the next sequence, the live record
/// count, the free list head, and the configuration the snapshot
/// was taken under.
struct SnapshotHeader {
    next_seq: u64,
    live_count: u32,
    config: BookConfig,
    free_list_head: u32,
}

/// Read and validate the 64-byte header: the fixed fields, the
/// size arithmetic against the byte length, and the sequence pair.
fn read_header(bytes: &[u8]) -> Result<SnapshotHeader, WalError> {
    let actual_len = bytes.len();
    if actual_len < HEADER_LEN + TRAILER_LEN {
        return Err(corrupt(SnapshotCorruption::SizeMismatch {
            expected: u64::try_from(HEADER_LEN + TRAILER_LEN).unwrap_or(0),
            actual: u64::try_from(actual_len).unwrap_or(0),
        }));
    }
    if bytes[0..4] != MAGIC {
        return Err(corrupt(SnapshotCorruption::BadMagic));
    }
    let version = read_u16(bytes, 4);
    if version != VERSION {
        return Err(corrupt(SnapshotCorruption::BadVersion { found: version }));
    }
    let header_len = read_u16(bytes, 6);
    if header_len != 64 {
        return Err(corrupt(SnapshotCorruption::BadHeaderLen {
            found: header_len,
        }));
    }
    let last_included_seq = read_u64(bytes, 8);
    let next_seq = read_u64(bytes, 16);
    let live_count = read_u32(bytes, 24);
    let max_orders = read_u32(bytes, 28);
    let tick_size = read_u64(bytes, 32);
    let grid_start_tick = read_u64(bytes, 40);
    let grid_len = read_u32(bytes, 48);
    let default_stp_raw = read_u32(bytes, 52);
    let free_list_head = read_u32(bytes, 56);
    let reserved = read_u32(bytes, 60);
    let expected_len =
        68u64 + 8 * u64::from(max_orders) + 20 * u64::from(grid_len) + 56 * u64::from(live_count);
    if expected_len != u64::try_from(actual_len).unwrap_or(0) {
        return Err(corrupt(SnapshotCorruption::SizeMismatch {
            expected: expected_len,
            actual: u64::try_from(actual_len).unwrap_or(0),
        }));
    }
    if last_included_seq.checked_add(1) != Some(next_seq) {
        return Err(corrupt(SnapshotCorruption::NextSeqMismatch {
            last_included_seq,
            next_seq,
        }));
    }
    if reserved != 0 {
        return Err(corrupt(SnapshotCorruption::NonZeroReserved {
            field: "header.reserved",
        }));
    }
    let default_stp = match default_stp_raw {
        0 => StpPolicy::CancelIncoming,
        1 => StpPolicy::CancelResting,
        2 => StpPolicy::CancelBoth,
        _ => {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "default_stp",
            }));
        }
    };
    Ok(SnapshotHeader {
        next_seq,
        live_count,
        config: BookConfig {
            max_orders,
            grid_len,
            tick_size,
            grid_start_tick,
            default_stp,
        },
        free_list_head,
    })
}

/// Decode the level section. Each level's list ends must be NIL
/// or name a slot.
fn decode_levels(
    bytes: &[u8],
    pos: &mut usize,
    grid_len: u32,
    max_orders: u32,
) -> Result<Vec<LevelState>, WalError> {
    let mut levels = Vec::with_capacity(grid_len as usize);
    for _ in 0..grid_len {
        let head = read_u32(bytes, *pos);
        let tail = read_u32(bytes, *pos + 4);
        if (head != u32::MAX && head >= max_orders) || (tail != u32::MAX && tail >= max_orders) {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "level list end",
            }));
        }
        levels.push(LevelState {
            head,
            tail,
            qty: read_u64(bytes, *pos + 8),
            count: read_u32(bytes, *pos + 16),
        });
        *pos += 20;
    }
    Ok(levels)
}

/// Decode and validate the live record section, returning the
/// records and which slots they mark live. Records are ascending
/// by slot, each agrees with its slot's persisted generation, and
/// each carries a NIL free link.
fn decode_live_records(
    bytes: &[u8],
    pos: &mut usize,
    header: &SnapshotHeader,
    generations: &[u32],
    free_links: &[u32],
) -> Result<(Vec<LiveOrderState>, Vec<bool>), WalError> {
    let max_orders = header.config.max_orders;
    let grid_len = header.config.grid_len;
    let mut live_orders = Vec::with_capacity(header.live_count as usize);
    let mut live_seen = vec![false; max_orders as usize];
    let mut previous_slot: Option<u32> = None;
    for _ in 0..header.live_count {
        let at = *pos;
        let slot = read_u32(bytes, at);
        let generation = read_u32(bytes, at + 4);
        let order_id = read_u64(bytes, at + 8);
        let participant = read_u32(bytes, at + 16);
        let price_idx = read_u32(bytes, at + 20);
        let qty = read_u64(bytes, at + 24);
        let side_byte = bytes[at + 32];
        let state_byte = bytes[at + 33];
        if bytes[at + 34..at + 40] != [0u8; 6] {
            return Err(corrupt(SnapshotCorruption::NonZeroReserved {
                field: "live record padding",
            }));
        }
        let prev = read_u32(bytes, at + 40);
        let next = read_u32(bytes, at + 44);
        if bytes[at + 48..at + 56] != [0u8; 8] {
            return Err(corrupt(SnapshotCorruption::NonZeroReserved {
                field: "live record reserved",
            }));
        }
        if slot >= max_orders {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "live record slot",
            }));
        }
        if let Some(previous) = previous_slot
            && slot <= previous
        {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "live record order",
            }));
        }
        if generation != generations[slot as usize] {
            return Err(corrupt(SnapshotCorruption::GenerationMismatch { slot }));
        }
        if free_links[slot as usize] != u32::MAX {
            return Err(corrupt(SnapshotCorruption::LiveSlotLinkNotNil { slot }));
        }
        let side = side_from_byte(side_byte).ok_or_else(|| {
            corrupt(SnapshotCorruption::InvalidField {
                field: "live record side",
            })
        })?;
        if state_byte != 1 {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "live record state",
            }));
        }
        if price_idx >= grid_len {
            return Err(corrupt(SnapshotCorruption::InvalidField {
                field: "live record price",
            }));
        }
        live_seen[slot as usize] = true;
        previous_slot = Some(slot);
        live_orders.push(LiveOrderState {
            slot,
            generation,
            order_id: OrderId(order_id),
            participant: ParticipantId(participant),
            price_idx,
            qty,
            side,
            prev,
            next,
        });
        *pos += 56;
    }
    Ok((live_orders, live_seen))
}

/// Walk the persisted free chain exactly (section 10.4): it
/// visits the free slots, each once, never a live slot, and every
/// remaining non-live slot must be a retired slot (generation at
/// the maximum, link NIL), which is how a snapshot tells free
/// from retired when the format carries no per-slot state section.
fn validate_free_list(
    header: &SnapshotHeader,
    generations: &[u32],
    free_links: &[u32],
    live_seen: &[bool],
) -> Result<(), WalError> {
    let max_orders = header.config.max_orders;
    let mut visited = vec![false; max_orders as usize];
    let mut cursor = header.free_list_head;
    while cursor != u32::MAX {
        if cursor >= max_orders {
            return Err(corrupt(SnapshotCorruption::FreeListInvalid {
                reason: "the chain leaves the slot range",
            }));
        }
        if live_seen[cursor as usize] {
            return Err(corrupt(SnapshotCorruption::FreeListInvalid {
                reason: "the chain visits a live slot",
            }));
        }
        if visited[cursor as usize] {
            return Err(corrupt(SnapshotCorruption::FreeListInvalid {
                reason: "the chain cycles",
            }));
        }
        visited[cursor as usize] = true;
        cursor = free_links[cursor as usize];
    }
    for slot in 0..max_orders {
        let s = slot as usize;
        if !live_seen[s] && !visited[s] {
            if generations[s] != u32::MAX {
                return Err(corrupt(SnapshotCorruption::FreeListInvalid {
                    reason: "a free slot is missing from the chain",
                }));
            }
            if free_links[s] != u32::MAX {
                return Err(corrupt(SnapshotCorruption::FreeListInvalid {
                    reason: "a retired slot carries a link",
                }));
            }
        }
    }
    Ok(())
}

/// Compare a snapshot's configuration against the caller's, field by
/// field, returning the mismatch error for the first field that
/// differs (section 10.4 and INV-SNAP-5). Shared by snapshot decoding
/// and by recovery, which receives an already decoded state.
pub fn config_mismatch(snapshot_config: &BookConfig, book_config: &BookConfig) -> Option<WalError> {
    let fields: [(&'static str, u64, u64); 5] = [
        (
            "max_orders",
            u64::from(snapshot_config.max_orders),
            u64::from(book_config.max_orders),
        ),
        (
            "tick_size",
            snapshot_config.tick_size,
            book_config.tick_size,
        ),
        (
            "grid_start_tick",
            snapshot_config.grid_start_tick,
            book_config.grid_start_tick,
        ),
        (
            "grid_len",
            u64::from(snapshot_config.grid_len),
            u64::from(book_config.grid_len),
        ),
        (
            "default_stp",
            u64::from(stp_to_byte(snapshot_config.default_stp)),
            u64::from(stp_to_byte(book_config.default_stp)),
        ),
    ];
    for (field, snapshot_value, book_value) in fields {
        if snapshot_value != book_value {
            return Some(WalError::SnapshotConfigMismatch {
                field,
                snapshot_value,
                book_value,
            });
        }
    }
    None
}

/// Shorthand for a corruption error.
const fn corrupt(detail: SnapshotCorruption) -> WalError {
    WalError::CorruptSnapshot { detail }
}

/// Little-endian u16 at byte `at`; the caller guarantees the width.
fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(
        bytes[at..at + 2]
            .try_into()
            .expect("caller guarantees two bytes"),
    )
}

/// Little-endian u32 at byte `at`; the caller guarantees the width.
fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        bytes[at..at + 4]
            .try_into()
            .expect("caller guarantees four bytes"),
    )
}

/// Little-endian u64 at byte `at`; the caller guarantees the width.
fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("caller guarantees eight bytes"),
    )
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
///
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
        let run = |book: &mut Book,
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
