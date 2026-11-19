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
//! surfaces the CLI actually runs (LogScanner, scan_reader, and
//! recover_from_reader) as pairs against their slice forms: on
//! every mutation the streaming verdict and outcome must equal
//! the slice form exactly, scan errors included.

use std::io;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use chronicle_wal::{
    LogScanner, Recovery, ScanOutcome, SyncPolicy, WalCommand, WalError, WalSink, WalWriter,
    decode_snapshot, encode_snapshot, recover, recover_from_reader, scan_log, scan_reader,
};

/// The default seed for the whole harness. The same seeded stream
/// drives every surface, so a failure reproduces exactly.
const SEED: u64 = 0xC0FF_EE15_600D_5EED;

/// The seed in force for this run (programme 3, lane T3). The
/// `CHRONICLE_MUTATION_SEED` environment variable overrides the
/// default, and the deep workflow derives a fresh one from its
/// run number on every dispatch, so successive deep runs explore
/// new streams instead of re-proving the default one. The seed is
/// printed in every panic message, so any failure reproduces by
/// setting the variable to the printed value.
fn harness_seed() -> u64 {
    std::env::var("CHRONICLE_MUTATION_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SEED)
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
                let detail = panic_detail(&payload);
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

/// Feed `iterations` seeded mutations of `base` to a slice surface
/// and its streaming counterpart as a pair (programme 4, lane U1).
/// The two forms must agree exactly: the same verdict, on success
/// the same outcome, and where `errors_must_match` holds, the same
/// rendered error. (Recovery is exempt from error equality
/// because the streaming form surfaces errors in stream order by
/// contract; the scan shares one parser, so its errors must be
/// identical.) A panic in either form, or any disagreement, fails
/// with the mutation's coordinates. Returns the counts of valid
/// and error outcomes.
fn run_streaming_pair<T: PartialEq>(
    base: &[u8],
    iterations: usize,
    surface: &str,
    errors_must_match: bool,
    mut slice: impl FnMut(&[u8]) -> Result<T, String>,
    mut stream: impl FnMut(&[u8]) -> Result<T, String>,
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
                    !errors_must_match || slice_error == stream_error,
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
                let detail = panic_detail(&payload);
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
    // same error, at chunk sizes 1, 7, and 64 and through
    // scan_reader over a cursor.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_streaming_pair(
        &wal,
        wal_cases(),
        "scan streaming",
        true,
        |b| scan_log(b).map_err(|e| e.to_string()),
        |b| {
            let mut agreed = scan_streamed(b, 1).map_err(|e| e.to_string())?;
            for chunk in [7usize, 64] {
                let next = scan_streamed(b, chunk).map_err(|e| e.to_string())?;
                assert_eq!(
                    next, agreed,
                    "chunk size {chunk} disagrees with chunk size 1"
                );
                agreed = next;
            }
            let via_reader = scan_reader(io::Cursor::new(b)).map_err(|e| e.to_string())?;
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
        |b| scan_log(b).map_err(|e| e.to_string()),
        |b| {
            let mut agreed = scan_streamed(b, 1).map_err(|e| e.to_string())?;
            for chunk in [7usize, 64] {
                let next = scan_streamed(b, chunk).map_err(|e| e.to_string())?;
                assert_eq!(
                    next, agreed,
                    "chunk size {chunk} disagrees with chunk size 1"
                );
                agreed = next;
            }
            let via_reader = scan_reader(io::Cursor::new(b)).map_err(|e| e.to_string())?;
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
        |b| {
            recover(config(), None, b)
                .map(|r| recovery_outcome(&r))
                .map_err(|e| e.to_string())
        },
        |b| {
            recover_from_reader(config(), None, io::Cursor::new(b))
                .map(|r| recovery_outcome(&r))
                .map_err(|e| e.to_string())
        },
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
        |b| {
            recover(config(), None, b)
                .map(|r| recovery_outcome(&r))
                .map_err(|e| e.to_string())
        },
        |b| {
            recover_from_reader(config(), None, io::Cursor::new(b))
                .map(|r| recovery_outcome(&r))
                .map_err(|e| e.to_string())
        },
        mutate_multi,
    );
    assert!(
        valid > 0,
        "flip-and-truncate cases must recover their prefix"
    );
    assert!(errors > 0, "corruptions must end in typed errors");
}
