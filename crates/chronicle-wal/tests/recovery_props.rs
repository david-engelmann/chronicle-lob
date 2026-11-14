//! Recovery equivalence properties (INV-WAL-5 and INV-SNAP-3).
//! Generated command streams run against a live book while every
//! accepted command is written to a log; recovery from the log, with
//! and without a midpoint snapshot, must reproduce the live book's
//! structural state exactly, and the two recovery paths must then
//! behave identically command by command.

use std::io;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    SnapshotState, StpPolicy,
};
use chronicle_wal::{SyncPolicy, WalCommand, WalSink, WalWriter, recover};
use proptest::prelude::*;

fn config() -> BookConfig {
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

proptest! {
    #[test]
    fn prop_wal_replay_reproduces_book(
        commands in prop::collection::vec(arb_command(), 0..48)
    ) {
        // INV-WAL-5: replaying the log of accepted commands into a
        // fresh book reproduces the live book exactly.
        let live = run_live(&commands, commands.len());
        let recovery = recover(config(), None, &live.wal).expect("recover");
        prop_assert_eq!(recovery.book.capture_snapshot_state(), live.final_state);
        prop_assert_eq!(recovery.report.frames_skipped, 0);
    }

    #[test]
    fn prop_snapshot_plus_wal_equals_full_replay(
        commands in prop::collection::vec(arb_command(), 1..48),
        post in prop::collection::vec(arb_command(), 0..24),
        split_seed in 0usize..1000,
    ) {
        // INV-SNAP-3: snapshot at a random midpoint plus the log
        // equals full replay, state by state, and both recovered
        // books then behave identically through a forced mass cancel,
        // a generated add phase that allocates from the restored free
        // list, and a second forced mass cancel, with byte-identical
        // event order.
        let split = split_seed % (commands.len() + 1);
        let live = run_live(&commands, split);
        let mut full = recover(config(), None, &live.wal)
            .expect("full replay")
            .book;
        let snapped = recover(config(), Some(live.mid_state.clone()), &live.wal)
            .expect("snapshot plus wal");
        prop_assert_eq!(snapped.report.frames_skipped, live.mid_accepted);
        let mut snapped = snapped.book;
        prop_assert_eq!(
            full.capture_snapshot_state(),
            live.final_state.clone()
        );
        prop_assert_eq!(
            snapped.capture_snapshot_state(),
            live.final_state
        );
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
    }
}
