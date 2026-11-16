// A worked durability session for chronicle-wal: five commands
// are written to a log as they are applied to a live book, a
// snapshot is taken halfway, and recovery from the pair must
// rebuild the live book exactly. The example writes session.wal
// and session.snapshot into the current directory, which is the
// pair the README's worked example inspects with chronicle-cli.
// CI runs this example and diffs its output against
// wal_recovery.expected.txt.

use std::fs;
use std::path::PathBuf;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use chronicle_wal::{SyncPolicy, WalCommand, WalWriter, load_snapshot, recover, save_snapshot};

const fn config() -> BookConfig {
    BookConfig {
        max_orders: 64,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

const fn add(order_id: u64, participant: u32, side: Side, price: u64, qty: u64) -> WalCommand {
    WalCommand::Add {
        order_id: OrderId(order_id),
        participant: ParticipantId(participant),
        side,
        order_type: OrderType::Limit,
        stp: None,
        price,
        qty,
    }
}

fn price_or_none(price: Option<(u64, u64)>) -> String {
    price.map_or_else(|| "none".to_string(), |(price, _qty)| price.to_string())
}

fn describe(label: &str, book: &Book) {
    println!(
        "{label}: live_orders={} best_bid={} best_ask={}",
        book.live_order_count(),
        price_or_none(book.best_bid()),
        price_or_none(book.best_ask())
    );
}

fn main() {
    let wal_path = PathBuf::from("session.wal");
    let snapshot_path = PathBuf::from("session.snapshot");
    let mut book = Book::new(config()).expect("the example configuration is valid");
    let file = fs::File::create(&wal_path).expect("create session.wal");
    let mut writer = WalWriter::new(file, SyncPolicy::Never);
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let commands = [
        add(1, 1, Side::Bid, 100, 10),
        add(2, 2, Side::Ask, 101, 5),
        add(3, 2, Side::Ask, 100, 4),
        WalCommand::Amend {
            order_id: OrderId(1),
            new_price: 99,
            new_qty: 6,
        },
        WalCommand::Cancel {
            order_id: OrderId(2),
        },
    ];
    let mut seq = 0u64;
    let mut snapshot_state = None;
    for (index, command) in commands.iter().enumerate() {
        {
            let mut events = EventBuffer::new(&mut storage);
            command
                .apply(&mut book, &mut events)
                .expect("session command applies");
        }
        storage.clear();
        seq += 1;
        writer.append(command, seq).expect("append frame");
        if index == 2 {
            let state = book.capture_snapshot_state();
            save_snapshot(&snapshot_path, &state).expect("save snapshot");
            snapshot_state = Some(state);
        }
    }
    drop(writer);
    let state = snapshot_state.expect("the snapshot is taken mid-session");
    println!("wrote: session.wal session.snapshot");
    println!(
        "snapshot: last_included_seq={} live_orders={}",
        state.next_seq - 1,
        state.live_orders.len()
    );
    let bytes = fs::read(&wal_path).expect("read session.wal");
    let loaded = load_snapshot(&snapshot_path, &config()).expect("the snapshot loads");
    assert_eq!(loaded, state, "the saved snapshot round-trips");
    let recovery = recover(config(), Some(&loaded), &bytes).expect("recovery succeeds");
    let report = recovery.report;
    println!(
        "recovery: frames_skipped={} frames_applied={} discarded={} last_recovered_seq={}",
        report.frames_skipped,
        report.frames_applied,
        report.discarded_trailing_bytes,
        report.last_recovered_seq
    );
    describe("recovered", &recovery.book);
    describe("live", &book);
    assert_eq!(recovery.book.live_order_count(), book.live_order_count());
    assert_eq!(recovery.book.best_bid(), book.best_bid());
    assert_eq!(recovery.book.best_ask(), book.best_ask());
    println!("match: yes");
}
