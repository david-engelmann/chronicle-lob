//! Decoder mutation harness (INV-WAL-7, roadmap lane R6).
//! Deterministic seeded mutations of valid WAL and snapshot bytes
//! are fed to the byte-level decoders: bit flips, byte overwrites,
//! truncations, and extensions, in a fixed rotation. Every outcome
//! must be a typed `WalError` or a valid decode. A panic at any
//! point fails the harness with the mutation's full coordinates,
//! and a panic found this way is a blocker whose minimised input
//! becomes a regression case, per the roadmap.

use std::io;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use chronicle_wal::{
    SyncPolicy, WalCommand, WalSink, WalWriter, decode_snapshot, encode_snapshot, recover, scan_log,
};

/// The one seed for the whole harness. The same seeded stream
/// drives every surface, so a failure reproduces exactly.
const SEED: u64 = 0xC0FF_EE15_600D_5EED;

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

/// SplitMix64, the same generator family the core drivers use,
/// small enough to carry in the harness itself.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
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

/// Feed `iterations` seeded mutations of `base` to one decoder
/// surface, which reports true for a valid decode and false for a
/// typed error. Returns the (valid, error) outcome counts.
fn run_surface(
    base: &[u8],
    iterations: usize,
    surface: &str,
    mut feed: impl FnMut(&[u8]) -> bool,
) -> (usize, usize) {
    let mut rng = SplitMix64::new(SEED);
    let mut valid = 0usize;
    let mut errors = 0usize;
    for i in 0..iterations {
        let kind = i % 4;
        let (bytes, offset) = mutate(base, &mut rng, kind);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| feed(&bytes)));
        match outcome {
            Ok(true) => valid += 1,
            Ok(false) => errors += 1,
            Err(_) => panic!(
                "decoder panicked: surface={surface} iteration={i} kind={kind} \
                 offset={offset} seed={SEED}"
            ),
        }
    }
    (valid, errors)
}

#[test]
fn mutation_harness_wal_never_panics() {
    // INV-WAL-7: 3,000 seeded mutations of a valid log against
    // both WAL surfaces. Truncations recover the prefix, so valid
    // outcomes must occur alongside typed errors.
    let (wal, _snapshot) = fixture_bytes();
    let (valid, errors) = run_surface(&wal, 3_000, "scan_log", |b| scan_log(b).is_ok());
    assert!(valid > 0, "truncations must decode as clean prefixes");
    assert!(errors > 0, "corruptions must end in typed errors");
    let (valid, errors) = run_surface(&wal, 3_000, "recover", |b| {
        recover(config(), None, b).is_ok()
    });
    assert!(valid > 0, "truncations must recover their prefix");
    assert!(errors > 0, "corruptions must end in typed errors");
}

#[test]
fn mutation_harness_snapshot_never_panics() {
    // INV-WAL-7: 1,500 seeded mutations of a valid snapshot
    // against the decoder. Any real change breaks a header check
    // or the CRC, so typed errors must occur; the point of the
    // harness is that nothing panics on the way there.
    let (_wal, snapshot) = fixture_bytes();
    let (_valid, errors) = run_surface(&snapshot, 1_500, "decode_snapshot", |b| {
        decode_snapshot(b, &config()).is_ok()
    });
    assert!(errors > 0, "mutations must end in typed errors");
}
