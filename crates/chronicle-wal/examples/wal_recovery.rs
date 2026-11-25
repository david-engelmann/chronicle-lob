//! A worked durability session for chronicle-wal: five commands
//! are written to a log as they are applied to a live book, a
//! snapshot is taken halfway, and recovery from the pair must
//! rebuild the live book exactly. A second act then runs the
//! checkpoint composition of ARCHITECTURE.md section 10.6:
//! stream-recover, snapshot, verify by loading back, archive the
//! covered log, start a fresh log, and recover across the seam.
//! Temporary files live inside a dedicated directory under the
//! system temp path and are removed before exit. CI runs this
//! example and diffs its output against `wal_recovery.expected.txt`.

use std::fs;
use std::path::{Path, PathBuf};

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use chronicle_wal::{
    SyncPolicy, WalCommand, WalWriter, load_snapshot, recover, recover_from_files, save_snapshot,
};

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
    let dir = std::env::temp_dir().join(format!("chronicle-wal-example-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create example scratch dir");

    let wal_path = dir.join("session.wal");
    let snapshot_path = dir.join("session.snapshot");
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
    let recovery = recover(config(), Some(loaded), &bytes).expect("recovery succeeds");
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
    checkpoint_act(&dir, &wal_path, &mut storage);
    fs::remove_dir_all(&dir).expect("clean up example scratch dir");
}

// The checkpoint composition (ARCHITECTURE.md section 10.6).
fn checkpoint_act(dir: &Path, wal_path: &Path, storage: &mut Vec<ExecutionEvent>) {
    let checkpoint_wal = dir.join("session-checkpoint.wal");
    fs::copy(wal_path, &checkpoint_wal).expect("copy the session log");
    let checkpoint_path = dir.join("session.checkpoint");
    let streamed = recover_from_files(config(), None, &checkpoint_wal)
        .expect("streaming recovery of the copied log succeeds");
    let checkpoint_state = streamed.book.capture_snapshot_state();
    save_snapshot(&checkpoint_path, &checkpoint_state).expect("save the checkpoint snapshot");
    let verified = load_snapshot(&checkpoint_path, &config()).expect("the checkpoint loads");
    assert_eq!(verified, checkpoint_state, "the checkpoint round-trips");
    let covered = checkpoint_state.next_seq - 1;
    let archive_name = format!("session-checkpoint.wal.archive-{covered}");
    let archive_path: PathBuf = dir.join(&archive_name);
    fs::rename(&checkpoint_wal, &archive_path).expect("archive the covered log");
    println!(
        "checkpoint: last_included_seq={} live_orders={} archived={archive_name}",
        covered,
        checkpoint_state.live_orders.len(),
    );

    // The embedding application then starts a fresh log at the
    // snapshot's next sequence, and recovery across the seam (the
    // checkpoint snapshot plus the fresh log) rebuilds the
    // continued book exactly.
    let mut continued = streamed.book;
    let file = fs::File::create(&checkpoint_wal).expect("create the fresh log");
    let mut fresh_writer = WalWriter::new(file, SyncPolicy::Never);
    let next = add(4, 1, Side::Bid, 98, 3);
    {
        let mut events = EventBuffer::new(&mut *storage);
        next.apply(&mut continued, &mut events)
            .expect("continuation applies");
    }
    storage.clear();
    fresh_writer
        .append(&next, covered + 1)
        .expect("append continuation frame");
    drop(fresh_writer);
    let across = recover_from_files(config(), Some(&checkpoint_path), &checkpoint_wal)
        .expect("recovery across the checkpoint seam succeeds");
    describe("continued", &continued);
    describe("recovered-after-checkpoint", &across.book);
    assert_eq!(across.book.live_order_count(), continued.live_order_count());
    assert_eq!(across.book.best_bid(), continued.best_bid());
    assert_eq!(across.book.best_ask(), continued.best_ask());
    println!("match: yes");
}
