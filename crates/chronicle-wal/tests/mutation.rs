//! Decoder mutation harness (INV-WAL-7, roadmap lane R6).
//! Deterministic seeded mutations of valid WAL and snapshot bytes
//! are fed to the byte-level decoders: bit flips, byte overwrites,
//! truncations, and extensions, in a fixed rotation. Every outcome
//! must be a typed `WalError` or a valid decode. A panic at any
//! point fails the harness with the mutation's full coordinates,
//! and a panic found this way is a blocker whose minimised input
//! becomes a regression case, per the roadmap.
//!
//! Programme 2 lane S6 adds multi-point mutations at the same
//! standard volumes (two value changes at distinct offsets, or a
//! value change paired with a truncation) and a dispatch-only deep
//! workflow (.github/workflows/mutation-deep.yml) that raises the
//! volume through the `CHRONICLE_MUTATION_CASES` environment
//! variable: the standard run uses 3,000 cases per WAL surface and
//! 1,500 per snapshot surface, and the deep run uses 100,000 and
//! 50,000.
//!
//! Programme 4 lane U1 points the same stream at the streaming
//! surfaces the CLI actually runs (`LogScanner`, `scan_reader`,
//! and `recover_from_reader`) as pairs against their slice forms:
//! on every mutation the streaming verdict and outcome must equal
//! the slice form, and a scan error's identity (its variant and
//! the offset it convicts) must match. The corrupt field a
//! multiply-corrupt header is diagnosed by may differ between
//! the forms, because the fragment path judges header bytes as
//! they arrive while the full-header path judges the payload
//! length first; the lane's first finding, recorded in
//! `error_key` below.

use std::io;
use std::sync::Once;

use chronicle_core::{
    Book, BookConfig, BookError, EventBuffer, ExecutionEvent, LevelState, LiveOrderState, OrderId,
    OrderType, ParticipantId, Side, SnapshotState, StpPolicy,
};
use chronicle_wal::{
    Crc32c, FrameParse, HEADER_LEN, LogScanner, MAX_PAYLOAD_LEN, Recovery, ScanOutcome,
    SnapshotCorruption, SyncPolicy, WalCommand, WalCorruption, WalError, WalSink, WalWriter,
    decode_snapshot, encode_frame, encode_snapshot, parse_frame, recover, recover_from_reader,
    scan_log, scan_reader, snapshot_config,
};

/// The default seed for the whole harness. The same seeded stream
/// drives every surface, so a failure reproduces exactly.
const SEED: u64 = 0xC0FF_EE15_600D_5EED;

static SEED_BANNER: Once = Once::new();

/// The seed in force for this run (programme 3, lane T3). The
/// `CHRONICLE_MUTATION_SEED` environment variable overrides
/// `SIM_SEED`, which in turn overrides the default. The active seed
/// is printed once per test binary and included in every panic
/// message, so any failure reproduces by setting the variable to the
/// printed value.
fn harness_seed() -> u64 {
    let seed = std::env::var("CHRONICLE_MUTATION_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            std::env::var("SIM_SEED")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(SEED);
    SEED_BANNER.call_once(|| {
        eprintln!("CHRONICLE_MUTATION_SEED={seed}");
    });
    seed
}

/// Mutation cases per WAL surface. The deep workflow raises the
/// standard volume through the environment; anything unreadable
/// falls back to the standard count.
fn wal_cases() -> usize {
    std::env::var("CHRONICLE_MUTATION_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3_000)
}

/// Mutation cases per snapshot surface, half the WAL volume.
fn snapshot_cases() -> usize {
    wal_cases() / 2
}

const fn config() -> BookConfig {
    BookConfig {
        max_orders: 32,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// A sink that keeps the log bytes in memory.
#[derive(Default)]
struct VecSink {
    bytes: Vec<u8>,
}

impl io::Write for VecSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WalSink for VecSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `SplitMix64`, the same generator family the core drivers use,
/// small enough to carry in the harness itself.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        let bound = u64::try_from(n).expect("bound fits u64");
        usize::try_from(self.next_u64() % bound).expect("remainder fits usize")
    }

    fn byte(&mut self) -> u8 {
        u8::try_from(self.next_u64() % 256).expect("value fits a byte")
    }
}

/// The five commands of the fixture session the CLI tests drive.
fn fixture_commands() -> Vec<WalCommand> {
    vec![
        WalCommand::Add {
            order_id: OrderId(1),
            participant: ParticipantId(1),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 10,
        },
        WalCommand::Add {
            order_id: OrderId(2),
            participant: ParticipantId(2),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 101,
            qty: 5,
        },
        WalCommand::Add {
            order_id: OrderId(3),
            participant: ParticipantId(2),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 4,
        },
        WalCommand::Amend {
            order_id: OrderId(1),
            new_price: 99,
            new_qty: 6,
        },
        WalCommand::Cancel {
            order_id: OrderId(2),
        },
    ]
}

/// Run the fixture session live, logging as it goes, and return
/// the valid WAL bytes and the valid snapshot bytes of the final
/// state.
fn fixture_bytes() -> (Vec<u8>, Vec<u8>) {
    let mut book = Book::new(config()).expect("book");
    let mut writer = WalWriter::new(VecSink::default(), SyncPolicy::Never);
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    for command in fixture_commands() {
        let seq = book.next_seq();
        let applied = {
            let mut events = EventBuffer::new(&mut storage);
            command.apply(&mut book, &mut events)
        };
        applied.expect("fixture command applies");
        writer.append(&command, seq).expect("append");
        storage.clear();
    }
    let state = book.capture_snapshot_state();
    let mut snapshot = Vec::new();
    encode_snapshot(&state, &mut snapshot);
    (writer.into_inner().bytes, snapshot)
}

/// Apply one mutation of the given kind (0 bit flip, 1 byte
/// overwrite, 2 truncation, 3 extension) and return the mutated
/// bytes with the offset the mutation centred on.
fn mutate(base: &[u8], rng: &mut SplitMix64, kind: usize) -> (Vec<u8>, usize) {
    let mut bytes = base.to_vec();
    let offset = rng.below(base.len());
    match kind {
        0 => bytes[offset] ^= 1u8 << (rng.next_u64() % 8),
        1 => bytes[offset] = rng.byte(),
        2 => bytes.truncate(offset.max(1)),
        _ => {
            let extra = 1 + rng.below(32);
            for _ in 0..extra {
                bytes.push(rng.byte());
            }
        }
    }
    (bytes, offset)
}

/// One mutation step: base bytes, generator, and kind in,
/// mutated bytes and centre offset out.
type MutateFn = fn(&[u8], &mut SplitMix64, usize) -> (Vec<u8>, usize);

/// Apply a multi-point mutation (lane S6): two value changes at
/// distinct offsets, or a value change paired with a truncation,
/// in a fixed rotation by kind (0 two bit flips, 1 two byte
/// overwrites, 2 a flip and an overwrite, 3 a flip and a
/// truncation). Returns the mutated bytes with the first offset.
fn mutate_multi(base: &[u8], rng: &mut SplitMix64, kind: usize) -> (Vec<u8>, usize) {
    const fn flip(bytes: &mut [u8], rng: &mut SplitMix64, offset: usize) {
        bytes[offset] ^= 1u8 << (rng.next_u64() % 8);
    }
    fn overwrite(bytes: &mut [u8], rng: &mut SplitMix64, offset: usize) {
        bytes[offset] = rng.byte();
    }
    fn distinct_offset(rng: &mut SplitMix64, len: usize, first: usize) -> usize {
        let mut other = rng.below(len);
        while other == first {
            other = rng.below(len);
        }
        other
    }
    let mut bytes = base.to_vec();
    let first = rng.below(base.len());
    match kind {
        0 => {
            flip(&mut bytes, rng, first);
            let second = distinct_offset(rng, base.len(), first);
            flip(&mut bytes, rng, second);
        }
        1 => {
            overwrite(&mut bytes, rng, first);
            let second = distinct_offset(rng, base.len(), first);
            overwrite(&mut bytes, rng, second);
        }
        2 => {
            flip(&mut bytes, rng, first);
            let second = distinct_offset(rng, base.len(), first);
            overwrite(&mut bytes, rng, second);
        }
        _ => {
            flip(&mut bytes, rng, first);
            let cut = rng.below(base.len());
            bytes.truncate(cut.max(1));
        }
    }
    (bytes, first)
}

/// Feed `iterations` seeded mutations of `base` to one decoder
/// surface, which reports true for a valid decode and false for a
/// typed error. Returns the (valid, error) outcome counts.
fn run_surface(
    base: &[u8],
    iterations: usize,
    surface: &str,
    mut feed: impl FnMut(&[u8]) -> bool,
    mutate_fn: MutateFn,
) -> (usize, usize) {
    let seed = harness_seed();
    let mut rng = SplitMix64::new(seed);
    let mut valid = 0usize;
    let mut errors = 0usize;
    for i in 0..iterations {
        let kind = i % 4;
        let (bytes, offset) = mutate_fn(base, &mut rng, kind);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| feed(&bytes)));
        match outcome {
            Ok(true) => valid += 1,
            Ok(false) => errors += 1,
            Err(payload) => {
                let detail = panic_detail(&*payload);
                panic!(
                    "decoder panicked: surface={surface} iteration={i} kind={kind} \
                     offset={offset} seed={seed}: {detail}"
                );
            }
        }
    }
    (valid, errors)
}

/// Extract a panic payload's message for the harness reports.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Scan `bytes` through the streaming scanner at one chunk size,
/// the way `scan_reader` drives it over a reader.
fn scan_streamed(bytes: &[u8], chunk: usize) -> Result<ScanOutcome, WalError> {
    let mut scanner = LogScanner::new();
    let mut frames = Vec::new();
    for part in bytes.chunks(chunk) {
        scanner.feed(part, &mut frames)?;
    }
    let discarded_trailing_bytes = scanner.finish(&mut frames)?;
    Ok(ScanOutcome {
        frames,
        discarded_trailing_bytes,
    })
}

/// The canonical outcome of one recovery, compared between the
/// slice and streaming forms: the report fields plus the encoded
/// snapshot bytes of the recovered book, so the forms agree only
/// if they agree on everything observable.
type RecoveryOutcome = (Option<u64>, usize, usize, usize, u64, Vec<u8>);

/// Render one recovery to its canonical outcome.
fn recovery_outcome(recovery: &Recovery) -> RecoveryOutcome {
    let report = &recovery.report;
    let mut encoded = Vec::new();
    encode_snapshot(&recovery.book.capture_snapshot_state(), &mut encoded);
    (
        report.snapshot_seq,
        report.frames_skipped,
        report.frames_applied,
        report.discarded_trailing_bytes,
        report.last_recovered_seq,
        encoded,
    )
}

/// The contractual identity of a scanning error: its variant and
/// the offset it convicts, plus a sequence gap's expected and
/// found values. The corrupt-record detail is excluded, on the
/// evidence of lane U1's first finding: with a header corrupt in
/// both its payload length and its reserved field, the slice form
/// names the length (the full-header path checks it first) while
/// the streaming form names the reserved field (the fragment path
/// judges those bytes as they arrive, before the header is whole).
/// Both are the same corruption at the same offset, and which
/// field is named is the one diagnostic that is window-dependent.
fn error_key(error: &WalError) -> String {
    match error {
        WalError::CorruptWalRecord { offset, .. } => format!("corrupt@{offset}"),
        WalError::WalSequenceGap {
            expected,
            found,
            offset,
        } => format!("gap@{offset}:{expected}>{found}"),
        other => format!("{other}"),
    }
}

/// Feed `iterations` seeded mutations of `base` to a slice surface
/// and its streaming counterpart as a pair (programme 4, lane U1).
/// The two forms must agree: the same verdict, on success the same
/// outcome, and where `errors_must_match` holds, the same error
/// identity per `error_key`. (Recovery is exempt from error
/// equality because the streaming form surfaces errors in stream
/// order by contract.) A panic in either form, or any
/// disagreement, fails with the mutation's coordinates. Returns
/// the counts of valid and error outcomes.
fn run_streaming_pair<T: PartialEq>(
    base: &[u8],
    iterations: usize,
    surface: &str,
    errors_must_match: bool,
    mut slice: impl FnMut(&[u8]) -> Result<T, WalError>,
    mut stream: impl FnMut(&[u8]) -> Result<T, WalError>,
    mutate_fn: MutateFn,
) -> (usize, usize) {
    let seed = harness_seed();
    let mut rng = SplitMix64::new(seed);
    let mut valid = 0usize;
    let mut errors = 0usize;
    for i in 0..iterations {
        let kind = i % 4;
        let (bytes, offset) = mutate_fn(base, &mut rng, kind);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (slice(&bytes), stream(&bytes))
        }));
        match outcome {
            Ok((Ok(slice_outcome), Ok(stream_outcome))) => {
                assert!(
                    slice_outcome == stream_outcome,
                    "streaming outcome differs from the slice outcome: surface={surface} \
                     iteration={i} kind={kind} offset={offset} seed={seed}"
                );
                valid += 1;
            }
            Ok((Err(slice_error), Err(stream_error))) => {
                assert!(
                    !errors_must_match || error_key(&slice_error) == error_key(&stream_error),
                    "streaming error differs from the slice error: surface={surface} \
                     iteration={i} kind={kind} offset={offset} seed={seed}: \
                     slice={slice_error} stream={stream_error}"
                );
                errors += 1;
            }
            Ok((slice_result, stream_result)) => {
                panic!(
                    "streaming and slice verdicts disagree: surface={surface} \
                     iteration={i} kind={kind} offset={offset} seed={seed} \
                     slice_ok={} stream_ok={}",
                    slice_result.is_ok(),
                    stream_result.is_ok()
                );
            }
            Err(payload) => {
                let detail = panic_detail(&*payload);
                panic!(
                    "decoder panicked: surface={surface} iteration={i} kind={kind} \
                     offset={offset} seed={seed}: {detail}"
                );
            }
        }
    }
    (valid, errors)
}

#[test]
fn mutation_harness_wal_never_panics() {
    // INV-WAL-7: seeded mutations of a valid log against both WAL
    // surfaces, 3,000 at the standard volume. Truncations recover
    // the prefix, so valid outcomes must occur alongside typed
    // errors.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_surface(
        &wal,
        wal_cases(),
        "scan_log",
        |b| scan_log(b).is_ok(),
        mutate,
    );
    assert!(valid > 0, "truncations must decode as clean prefixes");
    assert!(errors > 0, "corruptions must end in typed errors");
    let (valid, errors) = run_surface(
        &wal,
        wal_cases(),
        "recover",
        |b| recover(config(), None, b).is_ok(),
        mutate,
    );
    assert!(valid > 0, "truncations must recover their prefix");
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_snapshot_never_panics() {
    // INV-WAL-7: seeded mutations of a valid snapshot against the
    // decoder, 1,500 at the standard volume. Any real change
    // breaks a header check or the CRC, so typed errors must
    // occur; the point of the harness is that nothing panics on
    // the way there.
    let (_wal, snapshot) = fixture_bytes();
    let (_valid, errors) = run_surface(
        &snapshot,
        snapshot_cases(),
        "decode_snapshot",
        |b| decode_snapshot(b, &config()).is_ok(),
        mutate,
    );
    assert!(errors > 0, "mutations must end in typed errors");
}

#[test]
fn mutation_harness_wal_multi_point_never_panics() {
    // INV-WAL-7, lane S6: multi-point mutations of a valid log at
    // the same standard volume. The flip-and-truncate kind leaves
    // a clean prefix whenever the cut lands before the flip, so
    // valid outcomes must occur alongside typed errors.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_surface(
        &wal,
        wal_cases(),
        "scan_log multi-point",
        |b| scan_log(b).is_ok(),
        mutate_multi,
    );
    assert!(
        valid > 0,
        "flip-and-truncate cases must decode as clean prefixes"
    );
    assert!(errors > 0, "corruptions must end in typed errors");
    let (valid, errors) = run_surface(
        &wal,
        wal_cases(),
        "recover multi-point",
        |b| recover(config(), None, b).is_ok(),
        mutate_multi,
    );
    assert!(
        valid > 0,
        "flip-and-truncate cases must recover their prefix"
    );
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_snapshot_multi_point_never_panics() {
    // INV-WAL-7, lane S6: multi-point mutations of a valid
    // snapshot at the standard volume. Two changed points break a
    // header check or the CRC at least as surely as one, so typed
    // errors must occur and nothing may panic on the way there.
    let (_wal, snapshot) = fixture_bytes();
    let (_valid, errors) = run_surface(
        &snapshot,
        snapshot_cases(),
        "decode_snapshot multi-point",
        |b| decode_snapshot(b, &config()).is_ok(),
        mutate_multi,
    );
    assert!(errors > 0, "mutations must end in typed errors");
}

#[test]
fn mutation_harness_streaming_scan_matches_slice() {
    // INV-WAL-7, lane U1: the streaming scanner the CLI's verify
    // and replay actually run must agree with scan_log on every
    // mutation: same verdict, same frames, same discarded count,
    // same error identity, at chunk sizes 1, 7, and 64 and through
    // scan_reader over a cursor.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_streaming_pair(
        &wal,
        wal_cases(),
        "scan streaming",
        true,
        scan_log,
        |b| {
            let mut agreed = scan_streamed(b, 1)?;
            for chunk in [7usize, 64] {
                let next = scan_streamed(b, chunk)?;
                assert_eq!(
                    next, agreed,
                    "chunk size {chunk} disagrees with chunk size 1"
                );
                agreed = next;
            }
            let via_reader = scan_reader(io::Cursor::new(b))?;
            assert_eq!(via_reader, agreed, "scan_reader disagrees with the scanner");
            Ok(agreed)
        },
        mutate,
    );
    assert!(valid > 0, "truncations must decode as clean prefixes");
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_streaming_scan_multi_point_matches_slice() {
    // INV-WAL-7, lane U1: the multi-point stream against the same
    // streaming scan pair.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_streaming_pair(
        &wal,
        wal_cases(),
        "scan streaming multi-point",
        true,
        scan_log,
        |b| {
            let mut agreed = scan_streamed(b, 1)?;
            for chunk in [7usize, 64] {
                let next = scan_streamed(b, chunk)?;
                assert_eq!(
                    next, agreed,
                    "chunk size {chunk} disagrees with chunk size 1"
                );
                agreed = next;
            }
            let via_reader = scan_reader(io::Cursor::new(b))?;
            assert_eq!(via_reader, agreed, "scan_reader disagrees with the scanner");
            Ok(agreed)
        },
        mutate_multi,
    );
    assert!(
        valid > 0,
        "flip-and-truncate cases must decode as clean prefixes"
    );
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_streaming_recovery_matches_slice() {
    // INV-WAL-7, lane U1: streaming recovery must agree with the
    // slice form on every mutation: same verdict, and on success
    // the same report and the same recovered book. The rendered
    // errors may differ, because the streaming form surfaces them
    // in stream order by contract.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_streaming_pair(
        &wal,
        wal_cases(),
        "recover streaming",
        false,
        |b| recover(config(), None, b).map(|r| recovery_outcome(&r)),
        |b| recover_from_reader(config(), None, io::Cursor::new(b)).map(|r| recovery_outcome(&r)),
        mutate,
    );
    assert!(valid > 0, "truncations must recover their prefix");
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_streaming_recovery_multi_point_matches_slice() {
    // INV-WAL-7, lane U1: the multi-point stream against the same
    // streaming recovery pair.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_streaming_pair(
        &wal,
        wal_cases(),
        "recover streaming multi-point",
        false,
        |b| recover(config(), None, b).map(|r| recovery_outcome(&r)),
        |b| recover_from_reader(config(), None, io::Cursor::new(b)).map(|r| recovery_outcome(&r)),
        mutate_multi,
    );
    assert!(
        valid > 0,
        "flip-and-truncate cases must recover their prefix"
    );
    assert!(errors > 0, "corruptions must end in typed errors");
}

/// Recompute the domain-separated CRC32C of one WAL frame in place
/// when `frame` is large enough to hold its declared payload.
fn recompute_single_frame_crc(frame: &mut [u8]) {
    if frame.len() < HEADER_LEN {
        return;
    }
    let payload_len = u32::from_le_bytes([frame[16], frame[17], frame[18], frame[19]]);
    if payload_len > MAX_PAYLOAD_LEN {
        return;
    }
    let end = HEADER_LEN + payload_len as usize;
    if frame.len() < end {
        return;
    }
    let mut crc = Crc32c::new();
    crc.update(b"chronicle-wal/v1");
    crc.update(&[0]);
    crc.update(&frame[0..28]);
    crc.update(&frame[HEADER_LEN..end]);
    frame[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
}

/// Walk `wal` from offset 0 and recompute the stored CRC32C on every
/// complete frame whose `payload_len` fits inside the remaining slice,
/// so mutations bypass checksum rejection and reach structural header,
/// payload, sequence, and replay validation directly.
fn recompute_wal_crcs(wal: &mut [u8]) {
    let mut pos = 0usize;
    while pos + HEADER_LEN <= wal.len() {
        let payload_len =
            u32::from_le_bytes([wal[pos + 16], wal[pos + 17], wal[pos + 18], wal[pos + 19]]);
        if payload_len > MAX_PAYLOAD_LEN {
            break;
        }
        let frame_len = HEADER_LEN + payload_len as usize;
        if pos + frame_len > wal.len() {
            break;
        }
        recompute_single_frame_crc(&mut wal[pos..pos + frame_len]);
        pos += frame_len;
    }
}

/// Recompute the domain-separated snapshot trailer CRC32C in place,
/// so header and body mutations bypass checksum rejection and hit
/// every structural check in `snapshot_config`, `decode_snapshot`,
/// and `Book::from_snapshot_state`.
fn recompute_snapshot_crc(snap: &mut [u8]) {
    if snap.len() < 4 {
        return;
    }
    let body_end = snap.len() - 4;
    let mut crc = Crc32c::new();
    crc.update(b"chronicle-snapshot/v1");
    crc.update(&[0]);
    crc.update(&snap[..body_end]);
    snap[body_end..].copy_from_slice(&crc.finalize().to_le_bytes());
}

/// Exercise every command path (crossing sweep, Fok preflight, amend,
/// cancel, mass cancel) against a recovered or restored book to prove
/// it is structurally sound and free of cycles.
fn exercise_post_restore_replay(book: &mut Book) {
    book.debug_assert_invariants();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(128);
    let follow_ups = [
        WalCommand::Add {
            order_id: OrderId(50_001),
            participant: ParticipantId(9),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 120,
            qty: 25,
        },
        WalCommand::Add {
            order_id: OrderId(50_002),
            participant: ParticipantId(10),
            side: Side::Ask,
            order_type: OrderType::Fok,
            stp: None,
            price: 80,
            qty: 5,
        },
        WalCommand::Amend {
            order_id: OrderId(50_001),
            new_price: 105,
            new_qty: 10,
        },
        WalCommand::Cancel {
            order_id: OrderId(50_001),
        },
        WalCommand::CancelByParticipant {
            participant: ParticipantId(1),
        },
    ];
    for cmd in &follow_ups {
        let mut events = EventBuffer::new(&mut storage);
        let _ = cmd.apply(book, &mut events);
        storage.clear();
    }
    book.debug_assert_invariants();
}

/// Build a richer snapshot with two resting bids at one level, an ask
/// on the opposite side, and churned generations/free-list links.
fn rich_snapshot_bytes() -> Vec<u8> {
    let mut book = Book::new(config()).expect("book");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let cmds = [
        WalCommand::Add {
            order_id: OrderId(10),
            participant: ParticipantId(1),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 8,
        },
        WalCommand::Add {
            order_id: OrderId(11),
            participant: ParticipantId(2),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 6,
        },
        WalCommand::Add {
            order_id: OrderId(12),
            participant: ParticipantId(3),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 102,
            qty: 9,
        },
        WalCommand::Add {
            order_id: OrderId(13),
            participant: ParticipantId(4),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 105,
            qty: 4,
        },
        WalCommand::Cancel {
            order_id: OrderId(13),
        },
    ];
    for cmd in &cmds {
        let mut events = EventBuffer::new(&mut storage);
        cmd.apply(&mut book, &mut events).expect("apply");
        storage.clear();
    }
    let mut out = Vec::new();
    encode_snapshot(&book.capture_snapshot_state(), &mut out);
    out
}

#[test]
fn mutation_harness_wal_valid_crc_never_panics() {
    let mut single_frame = Vec::new();
    encode_frame(&fixture_commands()[0], 1, &mut single_frame);
    let (_valid, errors) = run_surface(
        &single_frame,
        wal_cases(),
        "parse_frame valid-crc",
        |raw| {
            let mut bytes = raw.to_vec();
            recompute_single_frame_crc(&mut bytes);
            match parse_frame(&bytes, 0) {
                Ok(FrameParse::Frame { command, seq, .. }) => {
                    let mut book = Book::new(config()).expect("book");
                    let mut storage = Vec::with_capacity(64);
                    let mut events = EventBuffer::new(&mut storage);
                    if seq == 1 && command.apply(&mut book, &mut events).is_ok() {
                        book.debug_assert_invariants();
                    }
                    true
                }
                Ok(FrameParse::CrashBoundary { .. }) => true,
                Err(_) => false,
            }
        },
        mutate_multi,
    );
    assert!(errors > 0, "structural frame corruptions must be rejected");

    let (wal, _) = fixture_bytes();
    for mutate_fn in [mutate as MutateFn, mutate_multi as MutateFn] {
        let (_valid, errors) = run_surface(
            &wal,
            wal_cases(),
            "recover valid-crc",
            |raw| {
                let mut bytes = raw.to_vec();
                recompute_wal_crcs(&mut bytes);
                recover(config(), None, &bytes).is_ok_and(|mut rec| {
                    exercise_post_restore_replay(&mut rec.book);
                    true
                })
            },
            mutate_fn,
        );
        assert!(errors > 0, "structural log corruptions must be rejected");
    }
}

#[test]
fn mutation_harness_snapshot_valid_crc_never_panics() {
    let snap = rich_snapshot_bytes();
    for mutate_fn in [mutate as MutateFn, mutate_multi as MutateFn] {
        let (_valid, errors) = run_surface(
            &snap,
            snapshot_cases(),
            "decode_snapshot + from_snapshot_state + replay valid-crc",
            |raw| {
                let mut bytes = raw.to_vec();
                recompute_snapshot_crc(&mut bytes);
                if let Ok(inferred) = snapshot_config(&bytes) {
                    assert!(
                        inferred.validate().is_ok(),
                        "snapshot_config accepted an invalid BookConfig: {inferred:?}"
                    );
                }
                decode_snapshot(&bytes, &config()).is_ok_and(|state| {
                    let mut book = Book::from_snapshot_state(state)
                        .expect("a snapshot accepted by decode_snapshot must restore cleanly");
                    exercise_post_restore_replay(&mut book);
                    true
                })
            },
            mutate_fn,
        );
        assert!(
            errors > 0,
            "structural snapshot mutations with valid CRC must end in typed errors"
        );
    }
}

#[test]
fn regression_max_orders_zero_header_rejected() {
    // Regression (a): a snapshot header declaring max_orders = 0 with
    // matching byte length and a valid recomputed CRC32C must be
    // rejected by snapshot_config and decode_snapshot.
    let grid_len = 64u32;
    let total_len = 68usize + 20 * (grid_len as usize);
    let mut bytes = vec![0u8; total_len];
    bytes[0..4].copy_from_slice(b"CSNP");
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&64u16.to_le_bytes());
    bytes[8..16].copy_from_slice(&0u64.to_le_bytes()); // last_included_seq
    bytes[16..24].copy_from_slice(&1u64.to_le_bytes()); // next_seq
    bytes[24..28].copy_from_slice(&0u32.to_le_bytes()); // live_count
    bytes[28..32].copy_from_slice(&0u32.to_le_bytes()); // max_orders = 0!
    bytes[32..40].copy_from_slice(&1u64.to_le_bytes()); // tick_size
    bytes[40..48].copy_from_slice(&1u64.to_le_bytes()); // grid_start_tick
    bytes[48..52].copy_from_slice(&grid_len.to_le_bytes());
    bytes[52..56].copy_from_slice(&0u32.to_le_bytes()); // default_stp
    bytes[56..60].copy_from_slice(&u32::MAX.to_le_bytes()); // free_list_head = NIL
    for i in 0..(grid_len as usize) {
        let at = 64 + i * 20;
        bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    recompute_snapshot_crc(&mut bytes);

    let err = snapshot_config(&bytes).expect_err("max_orders = 0 must fail snapshot_config");
    assert!(matches!(
        err,
        WalError::CorruptSnapshot {
            detail: SnapshotCorruption::InvalidField {
                field: "max_orders"
            }
        }
    ));

    let err = decode_snapshot(&bytes, &config()).expect_err("max_orders = 0 must fail decode");
    assert!(matches!(
        err,
        WalError::CorruptSnapshot {
            detail: SnapshotCorruption::InvalidField {
                field: "max_orders"
            }
        }
    ));
}

#[test]
fn regression_invalid_order_fields_and_links_rejected() {
    // Regression (b): order_id = 0, qty = 0, and out-of-bounds
    // prev/next/level links must be rejected with typed errors in both
    // debug and release builds, via decode_snapshot (with recomputed
    // CRC32C) and via Book::from_snapshot_state directly.
    let mut levels = vec![
        LevelState {
            head: u32::MAX,
            tail: u32::MAX,
            qty: 0,
            count: 0,
        };
        640
    ];
    levels[99] = LevelState {
        head: 0,
        tail: 0,
        qty: 10,
        count: 1,
    };
    let mut free_links = vec![u32::MAX; 32];
    for (i, link) in free_links.iter_mut().enumerate().skip(1) {
        if i + 1 < 32 {
            *link = u32::try_from(i + 1).unwrap();
        }
    }
    let valid_state = SnapshotState {
        config: config(),
        next_seq: 2,
        generations: vec![1; 32],
        free_list_head: 1,
        free_links,
        levels,
        live_orders: vec![LiveOrderState {
            slot: 0,
            generation: 1,
            order_id: OrderId(1),
            participant: ParticipantId(1),
            price_idx: 99,
            qty: 10,
            side: Side::Ask,
            prev: u32::MAX,
            next: u32::MAX,
        }],
    };
    let restored = Book::from_snapshot_state(valid_state.clone()).expect("baseline state is valid");
    restored.debug_assert_invariants();

    let bad_states: Vec<(&str, SnapshotState)> = vec![
        ("order_id = 0", {
            let mut s = valid_state.clone();
            s.live_orders[0].order_id = OrderId(0);
            s
        }),
        ("qty = 0", {
            let mut s = valid_state.clone();
            s.live_orders[0].qty = 0;
            s.levels[99].qty = 0;
            s
        }),
        ("prev out of bounds", {
            let mut s = valid_state.clone();
            s.live_orders[0].prev = 32;
            s
        }),
        ("next out of bounds", {
            let mut s = valid_state.clone();
            s.live_orders[0].next = 999;
            s
        }),
        ("level head out of bounds", {
            let mut s = valid_state.clone();
            s.levels[99].head = 32;
            s
        }),
        ("level tail out of bounds", {
            let mut s = valid_state;
            s.levels[99].tail = 32;
            s
        }),
    ];

    for (label, bad_state) in bad_states {
        let mut encoded = Vec::new();
        encode_snapshot(&bad_state, &mut encoded);
        let decode_err = decode_snapshot(&encoded, &config())
            .expect_err(&format!("{label}: decode_snapshot must reject"));
        assert!(
            matches!(decode_err, WalError::CorruptSnapshot { .. }),
            "{label}: unexpected decode error {decode_err:?}"
        );
        let restore_err = Book::from_snapshot_state(bad_state)
            .expect_err(&format!("{label}: from_snapshot_state must reject"));
        assert!(
            matches!(restore_err, BookError::InvalidSnapshot { .. }),
            "{label}: unexpected restore error {restore_err:?}"
        );
    }
}

#[test]
fn regression_self_loop_next_rejected_without_hanging_plan_sweep() {
    // Regression (c): a live resting order with a self-looping
    // `next = 0` must be rejected by both decode_snapshot (even with a
    // valid CRC32C) and Book::from_snapshot_state (even in --release),
    // so an incoming crossing sweep can never enter an infinite loop
    // inside plan_sweep.
    let mut book = Book::new(config()).expect("book");
    let mut storage = Vec::with_capacity(16);
    {
        let mut events = EventBuffer::new(&mut storage);
        book.add_order(
            OrderId(1),
            ParticipantId(2),
            Side::Ask,
            OrderType::Limit,
            100,
            10,
            None,
            &mut events,
        )
        .expect("rest ask");
    }
    let mut cyclic = book.capture_snapshot_state();
    assert_eq!(cyclic.live_orders[0].slot, 0);
    cyclic.live_orders[0].next = 0; // self-loop!

    let mut bytes = Vec::new();
    encode_snapshot(&cyclic, &mut bytes);
    let decode_err =
        decode_snapshot(&bytes, &config()).expect_err("cyclic snapshot must fail decode_snapshot");
    assert!(matches!(
        decode_err,
        WalError::CorruptSnapshot {
            detail: SnapshotCorruption::InvalidField {
                field: "level list cycle"
            }
        }
    ));

    let restore_err = Book::from_snapshot_state(cyclic)
        .expect_err("cyclic snapshot must fail from_snapshot_state in all build profiles");
    assert!(matches!(restore_err, BookError::InvalidSnapshot { .. }));
}

#[test]
fn regression_zero_price_at_grid_start_zero_rejected() {
    // When grid_start_tick == 0, tick 0 maps to raw price 0, which is
    // outside the positive price domain (min_price() == tick_size).
    // Both decode_snapshot (with valid CRC32C) and Book::from_snapshot_state
    // must reject a live order resting at price_idx == 0.
    let zero_start_cfg = BookConfig {
        max_orders: 8,
        grid_len: 64,
        tick_size: 5,
        grid_start_tick: 0,
        default_stp: StpPolicy::CancelIncoming,
    };
    let mut levels = vec![
        LevelState {
            head: u32::MAX,
            tail: u32::MAX,
            qty: 0,
            count: 0,
        };
        64
    ];
    levels[0] = LevelState {
        head: 0,
        tail: 0,
        qty: 10,
        count: 1,
    };
    let mut free_links = vec![u32::MAX; 8];
    for (i, link) in free_links.iter_mut().enumerate().skip(1) {
        if i + 1 < 8 {
            *link = u32::try_from(i + 1).unwrap();
        }
    }
    let bad_state = SnapshotState {
        config: zero_start_cfg,
        next_seq: 2,
        generations: vec![1; 8],
        free_list_head: 1,
        free_links,
        levels,
        live_orders: vec![LiveOrderState {
            slot: 0,
            generation: 1,
            order_id: OrderId(1),
            participant: ParticipantId(1),
            price_idx: 0,
            qty: 10,
            side: Side::Bid,
            prev: u32::MAX,
            next: u32::MAX,
        }],
    };

    let mut bytes = Vec::new();
    encode_snapshot(&bad_state, &mut bytes);
    let decode_err = decode_snapshot(&bytes, &zero_start_cfg)
        .expect_err("price_idx = 0 under grid_start_tick = 0 must fail decode_snapshot");
    assert!(matches!(
        decode_err,
        WalError::CorruptSnapshot {
            detail: SnapshotCorruption::InvalidField {
                field: "live record price"
            }
        }
    ));

    let restore_err = Book::from_snapshot_state(bad_state)
        .expect_err("price_idx = 0 under grid_start_tick = 0 must fail from_snapshot_state");
    assert!(matches!(restore_err, BookError::InvalidSnapshot { .. }));
}

#[test]
fn regression_zero_seq_wal_frame_rejected() {
    // Valid WAL sequence numbers are 1-based (seq >= 1). A crafted
    // seq = 0 frame with a valid CRC32C must be rejected as corruption
    // by parse_frame, scan_log, scan_reader, recover, and
    // recover_from_reader rather than silently skipped under base_seq = 0.
    let cmd = WalCommand::Add {
        order_id: OrderId(1),
        participant: ParticipantId(1),
        side: Side::Bid,
        order_type: OrderType::Limit,
        stp: None,
        price: 100,
        qty: 10,
    };
    let mut log = Vec::new();
    let mut frame = Vec::new();
    encode_frame(&cmd, 0, &mut frame);
    log.extend_from_slice(&frame);
    encode_frame(
        &WalCommand::Add {
            order_id: OrderId(2),
            participant: ParticipantId(1),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 105,
            qty: 10,
        },
        1,
        &mut frame,
    );
    log.extend_from_slice(&frame);

    for err in [
        parse_frame(&log, 0).expect_err("parse_frame must reject seq = 0"),
        scan_log(&log).expect_err("scan_log must reject seq = 0"),
        scan_reader(io::Cursor::new(&log)).expect_err("scan_reader must reject seq = 0"),
        recover(config(), None, &log).expect_err("recover must reject seq = 0"),
        recover_from_reader(config(), None, io::Cursor::new(&log))
            .expect_err("recover_from_reader must reject seq = 0"),
    ] {
        assert!(
            matches!(
                err,
                WalError::CorruptWalRecord {
                    offset: 0,
                    detail: WalCorruption::ZeroSequence,
                }
            ),
            "expected ZeroSequence at offset 0, got {err:?}"
        );
    }
}
