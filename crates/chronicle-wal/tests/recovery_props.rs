//! Recovery equivalence properties (INV-WAL-5 and INV-SNAP-3).
//! Generated command streams run against a live book while every
//! accepted command is written to a log; recovery from the log, with
//! and without a midpoint snapshot, must reproduce the live book's
//! structural state exactly, and the two recovery paths must then
//! behave identically command by command. Roadmap lane R4 adds the
//! byte-level pair: a captured state must survive snapshot encode
//! and decode unchanged (INV-SNAP-1), and recovery must still equal
//! the live book when the snapshot reaches it as encoded bytes
//! rather than as a live value (INV-SNAP-3).

use std::io;
use std::sync::Once;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    SnapshotState, StpPolicy,
};
use chronicle_wal::{
    SyncPolicy, WalCommand, WalSink, WalWriter, decode_snapshot, encode_snapshot, recover,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseResult, TestRng, TestRunner};

static SEED_BANNER: Once = Once::new();

fn derive_chacha_seed(sim_seed: u64, stream_id: &str) -> [u8; 32] {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in stream_id.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01B3);
    }
    let mut state = sim_seed ^ hash;
    let mut next_u64 = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut seed = [0u8; 32];
    for chunk in seed.as_chunks_mut::<8>().0 {
        *chunk = next_u64().to_le_bytes();
    }
    seed
}

fn run_seeded_proptest<S>(stream_id: &str, strategy: S, body: impl Fn(S::Value) -> TestCaseResult)
where
    S: Strategy,
{
    let sim_seed: u64 = std::env::var("SIM_SEED")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);
    let default_cfg = Config::default();
    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default_cfg.cases);
    SEED_BANNER.call_once(|| {
        eprintln!("SIM_SEED={sim_seed} PROPTEST_CASES={cases}");
    });
    let chacha_seed = derive_chacha_seed(sim_seed, stream_id);
    let config = Config {
        cases,
        failure_persistence: None,
        ..default_cfg
    };
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &chacha_seed);
    let mut runner = TestRunner::new_with_rng(config, rng);
    if let Err(err) = runner.run(&strategy, body) {
        panic!("property {stream_id} failed (SIM_SEED={sim_seed} PROPTEST_CASES={cases}): {err}");
    }
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

/// The outcome of running a command stream live: the log of accepted
/// commands, the states at the split point and at the end, and how
/// many commands were accepted before the split.
struct LiveRun {
    wal: Vec<u8>,
    mid_state: SnapshotState,
    mid_accepted: usize,
    final_state: SnapshotState,
}

fn run_live(commands: &[WalCommand], split: usize) -> LiveRun {
    let mut book = Book::new(config()).expect("book");
    let mut writer = WalWriter::new(VecSink::default(), SyncPolicy::Never);
    let mut mid_state = book.capture_snapshot_state();
    let mut mid_accepted = 0usize;
    let mut accepted = 0usize;
    for (i, command) in commands.iter().enumerate() {
        if i == split {
            mid_state = book.capture_snapshot_state();
            mid_accepted = accepted;
        }
        let seq = book.next_seq();
        let mut storage = Vec::with_capacity(256);
        let applied = {
            let mut events = EventBuffer::new(&mut storage);
            command.apply(&mut book, &mut events)
        };
        if applied.is_ok() {
            writer.append(command, seq).expect("append");
            accepted += 1;
        }
    }
    if split >= commands.len() {
        mid_state = book.capture_snapshot_state();
        mid_accepted = accepted;
    }
    LiveRun {
        wal: writer.into_inner().bytes,
        mid_state,
        mid_accepted,
        final_state: book.capture_snapshot_state(),
    }
}

/// Apply one command and return its events or its error, for
/// comparing two books command by command.
fn apply(
    book: &mut Book,
    command: &WalCommand,
) -> Result<Vec<ExecutionEvent>, chronicle_core::BookError> {
    let mut storage = Vec::with_capacity(256);
    let result = {
        let mut events = EventBuffer::new(&mut storage);
        command.apply(book, &mut events)
    };
    result.map(|_| storage)
}

fn arb_command() -> impl Strategy<Value = WalCommand> {
    prop_oneof![
        (
            1u64..=12,
            1u32..=3,
            prop::bool::ANY,
            0u8..4,
            95u64..=106,
            1u64..=12
        )
            .prop_map(|(id, participant, ask, kind, price, qty)| WalCommand::Add {
                order_id: OrderId(id),
                participant: ParticipantId(participant),
                side: if ask { Side::Ask } else { Side::Bid },
                order_type: match kind {
                    0 => OrderType::Limit,
                    1 => OrderType::Ioc,
                    2 => OrderType::Fok,
                    _ => OrderType::PostOnly,
                },
                stp: None,
                price,
                qty,
            }),
        (1u64..=12).prop_map(|id| WalCommand::Cancel {
            order_id: OrderId(id),
        }),
        (1u32..=3).prop_map(|participant| WalCommand::CancelByParticipant {
            participant: ParticipantId(participant),
        }),
        (1u64..=12, 95u64..=106, 1u64..=12).prop_map(|(id, price, qty)| WalCommand::Amend {
            order_id: OrderId(id),
            new_price: price,
            new_qty: qty,
        }),
    ]
}

#[test]
fn prop_wal_replay_reproduces_book() {
    // INV-WAL-5: replaying the log of accepted commands into a
    // fresh book reproduces the live book exactly.
    run_seeded_proptest(
        "prop_wal_replay_reproduces_book",
        prop::collection::vec(arb_command(), 0..48),
        |commands| {
            let live = run_live(&commands, commands.len());
            let recovery = recover(config(), None, &live.wal).expect("recover");
            prop_assert_eq!(recovery.book.capture_snapshot_state(), live.final_state);
            prop_assert_eq!(recovery.report.frames_skipped, 0);
            Ok(())
        },
    );
}

#[test]
fn prop_snapshot_plus_wal_equals_full_replay() {
    // INV-SNAP-3: snapshot at a random midpoint plus the log
    // equals full replay, state by state, and both recovered
    // books then behave identically through a forced mass cancel,
    // a generated add phase that allocates from the restored free
    // list, and a second forced mass cancel, with byte-identical
    // event order.
    run_seeded_proptest(
        "prop_snapshot_plus_wal_equals_full_replay",
        (
            prop::collection::vec(arb_command(), 1..48),
            prop::collection::vec(arb_command(), 0..24),
            0usize..1000,
        ),
        |(commands, post, split_seed)| {
            let split = split_seed % (commands.len() + 1);
            let live = run_live(&commands, split);
            let mut full = recover(config(), None, &live.wal)
                .expect("full replay")
                .book;
            let snapped = recover(config(), Some(live.mid_state.clone()), &live.wal)
                .expect("snapshot plus wal");
            prop_assert_eq!(snapped.report.frames_skipped, live.mid_accepted);
            let mut snapped = snapped.book;
            prop_assert_eq!(&full.capture_snapshot_state(), &live.final_state);
            prop_assert_eq!(&snapped.capture_snapshot_state(), &live.final_state);
            // Phase 1: forced mass cancel on both books.
            for participant in 1..=3u32 {
                let command = WalCommand::CancelByParticipant {
                    participant: ParticipantId(participant),
                };
                prop_assert_eq!(apply(&mut full, &command), apply(&mut snapped, &command));
            }
            prop_assert_eq!(
                full.capture_snapshot_state(),
                snapped.capture_snapshot_state()
            );
            // Phase 2: generated commands allocate from the restored
            // free list.
            for command in &post {
                prop_assert_eq!(apply(&mut full, command), apply(&mut snapped, command));
            }
            prop_assert_eq!(
                full.capture_snapshot_state(),
                snapped.capture_snapshot_state()
            );
            // Phase 3: a second forced mass cancel.
            for participant in 1..=3u32 {
                let command = WalCommand::CancelByParticipant {
                    participant: ParticipantId(participant),
                };
                prop_assert_eq!(apply(&mut full, &command), apply(&mut snapped, &command));
            }
            prop_assert_eq!(
                full.capture_snapshot_state(),
                snapped.capture_snapshot_state()
            );
            Ok(())
        },
    );
}

#[test]
fn prop_snapshot_bytes_round_trip() {
    // INV-SNAP-1 at the byte level (R4): a state captured from a
    // generated session survives encode and decode unchanged,
    // not only the golden book the layout test pins.
    run_seeded_proptest(
        "prop_snapshot_bytes_round_trip",
        (prop::collection::vec(arb_command(), 0..48), 0usize..1000),
        |(commands, split_seed)| {
            let split = split_seed % (commands.len() + 1);
            let live = run_live(&commands, split);
            let mut bytes = Vec::new();
            encode_snapshot(&live.mid_state, &mut bytes);
            let decoded = decode_snapshot(&bytes, &config()).expect("decode");
            prop_assert_eq!(decoded, live.mid_state);
            Ok(())
        },
    );
}

#[test]
fn prop_recovery_from_snapshot_bytes_equals_live() {
    // INV-SNAP-3 through the byte path (R4): the snapshot
    // reaches recovery as encoded bytes, decoded first, and the
    // recovered book still equals the live book exactly, with
    // the same frames skipped.
    run_seeded_proptest(
        "prop_recovery_from_snapshot_bytes_equals_live",
        (prop::collection::vec(arb_command(), 1..48), 0usize..1000),
        |(commands, split_seed)| {
            let split = split_seed % (commands.len() + 1);
            let live = run_live(&commands, split);
            let mut bytes = Vec::new();
            encode_snapshot(&live.mid_state, &mut bytes);
            let decoded = decode_snapshot(&bytes, &config()).expect("decode");
            let recovery = recover(config(), Some(decoded), &live.wal).expect("recover");
            prop_assert_eq!(recovery.report.frames_skipped, live.mid_accepted);
            prop_assert_eq!(recovery.book.capture_snapshot_state(), live.final_state);
            Ok(())
        },
    );
}
