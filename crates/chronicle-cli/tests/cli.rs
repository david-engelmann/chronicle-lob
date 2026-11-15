//! Behaviour tests for the chronicle-cli binary (ARCHITECTURE.md
//! section 2.3). The fixture drives a live book and a WAL writer in
//! lockstep, so the expected event stream is the engine's own output
//! formatted by the contract formatter in this file; the binary must
//! reproduce it from the files alone.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use chronicle_core::{
    Book, BookConfig, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, SnapshotState, StpPolicy,
};
use chronicle_wal::{SyncPolicy, WalCommand, WalWriter, load_snapshot, save_snapshot};

const BIN: &str = env!("CARGO_BIN_EXE_chronicle-cli");

const fn config() -> BookConfig {
    BookConfig {
        max_orders: 64,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

const CONFIG_FLAGS: [&str; 8] = [
    "--max-orders",
    "64",
    "--grid-len",
    "640",
    "--tick-size",
    "1",
    "--grid-start-tick",
    "1",
];

const fn reason_token(reason: CancelReason) -> &'static str {
    match reason {
        CancelReason::Requested => "requested",
        CancelReason::ByParticipant => "by-participant",
        CancelReason::SelfTradePrevention => "self-trade-prevention",
    }
}

/// The section 2.3 event line contract, one line per event.
fn format_event(event: &ExecutionEvent) -> String {
    match event {
        ExecutionEvent::Accepted { seq, order_id } => {
            format!("{seq} accepted order={}", order_id.0)
        }
        ExecutionEvent::Trade {
            seq,
            maker_order_id,
            taker_order_id,
            price,
            qty,
        } => format!(
            "{seq} trade maker={} taker={} price={price} qty={qty}",
            maker_order_id.0, taker_order_id.0
        ),
        ExecutionEvent::Filled { seq, order_id } => format!("{seq} filled order={}", order_id.0),
        ExecutionEvent::Cancelled {
            seq,
            order_id,
            reason,
        } => format!(
            "{seq} cancelled order={} reason={}",
            order_id.0,
            reason_token(*reason)
        ),
        ExecutionEvent::Amended {
            seq,
            order_id,
            price,
            qty,
        } => format!("{seq} amended order={} price={price} qty={qty}", order_id.0),
        ExecutionEvent::Expired {
            seq,
            order_id,
            remaining_qty,
        } => format!(
            "{seq} expired order={} remaining={remaining_qty}",
            order_id.0
        ),
        ExecutionEvent::SelfTradePrevented { seq, order_id } => {
            format!("{seq} self-trade-prevented order={}", order_id.0)
        }
    }
}

struct Session {
    dir: PathBuf,
    wal: PathBuf,
    snapshot: PathBuf,
    lines: Vec<String>,
    final_state: SnapshotState,
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("chronicle-cli-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
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

/// The fixture session: two resting orders, a crossing order that
/// trades and fills, an amend, and a cancel. A snapshot is taken
/// after the third command.
fn build_session(name: &str) -> Session {
    let dir = test_dir(name);
    let wal = dir.join("book.wal");
    let snapshot = dir.join("book.snapshot");
    let mut book = Book::new(config()).expect("book");
    let file = fs::File::create(&wal).expect("create wal");
    let mut writer = WalWriter::new(file, SyncPolicy::Never);
    let mut storage: Vec<ExecutionEvent> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut seq = 0u64;
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
    for (index, command) in commands.iter().enumerate() {
        let mut events = EventBuffer::new(&mut storage);
        let written = command
            .apply(&mut book, &mut events)
            .expect("session command applies");
        seq += 1;
        writer.append(command, seq).expect("append");
        for event in &storage[..written] {
            lines.push(format_event(event));
        }
        storage.clear();
        if index == 2 {
            let state = book.capture_snapshot_state();
            save_snapshot(&snapshot, &state).expect("save snapshot");
        }
    }
    drop(writer);
    let final_state = book.capture_snapshot_state();
    Session {
        dir,
        wal,
        snapshot,
        lines,
        final_state,
    }
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("run cli")
}

fn run_cli_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(BIN);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run cli")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf8")
}

fn joined(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn replay_prints_the_live_event_stream() {
    let session = build_session("replay-full");
    let mut args = vec!["replay", "--wal", session.wal.to_str().unwrap()];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout_of(&output), joined(&session.lines));
    assert_eq!(session.lines[0], "1 accepted order=1");
    assert!(
        session
            .lines
            .contains(&"3 trade maker=1 taker=3 price=100 qty=4".to_string()),
        "the crossing command trades at the maker price: {:?}",
        session.lines
    );
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn replay_from_snapshot_prints_only_later_events() {
    let session = build_session("replay-snap");
    let output = run_cli(&[
        "replay",
        "--wal",
        session.wal.to_str().unwrap(),
        "--snapshot",
        session.snapshot.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let later: Vec<String> = session
        .lines
        .iter()
        .filter(|line| {
            !(line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("3 "))
        })
        .cloned()
        .collect();
    assert_eq!(
        later.len(),
        2,
        "only the amend and the cancel follow the snapshot"
    );
    assert_eq!(stdout_of(&output), joined(&later));
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn verify_reports_a_healthy_log() {
    let session = build_session("verify-log");
    let mut args = vec!["verify", "--wal", session.wal.to_str().unwrap()];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert!(output.status.success());
    assert_eq!(
        stdout_of(&output),
        "wal: ok frames=5 discarded=0 last_seq=5\nverdict: ok\n"
    );
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn verify_reports_a_healthy_pair() {
    let session = build_session("verify-pair");
    let output = run_cli(&[
        "verify",
        "--wal",
        session.wal.to_str().unwrap(),
        "--snapshot",
        session.snapshot.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert_eq!(
        stdout_of(&output),
        "snapshot: ok last_included_seq=3 live_orders=2\nwal: ok frames=5 discarded=0 last_seq=5\nverdict: ok\n"
    );
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn verify_reports_corruption_with_exit_1() {
    let session = build_session("verify-corrupt");
    let mut bytes = fs::read(&session.wal).expect("read wal");
    // Inside the second frame's payload: the checksum must catch it.
    bytes[112] = bytes[112].wrapping_add(1);
    fs::write(&session.wal, &bytes).expect("write damaged wal");
    let mut args = vec!["verify", "--wal", session.wal.to_str().unwrap()];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert_eq!(output.status.code(), Some(1));
    let stdout = stdout_of(&output);
    assert!(
        stdout.starts_with("verdict: corrupt"),
        "stdout was: {stdout}"
    );
    assert!(!stdout.contains("wal: ok"), "stdout was: {stdout}");
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn snapshot_writes_a_loadable_snapshot() {
    let session = build_session("snapshot-cmd");
    let out_path = session.dir.join("written.snapshot");
    let mut args = vec![
        "snapshot",
        "--wal",
        session.wal.to_str().unwrap(),
        "--out",
        out_path.to_str().unwrap(),
    ];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout_of(&output),
        "snapshot: written last_included_seq=5 live_orders=1\n"
    );
    let loaded = load_snapshot(&out_path, &config()).expect("written snapshot loads");
    assert_eq!(loaded, session.final_state);
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn config_flags_are_required_without_a_snapshot() {
    let session = build_session("flags-required");
    let output = run_cli(&["replay", "--wal", session.wal.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn config_flags_conflict_with_a_snapshot() {
    let session = build_session("flags-conflict");
    let output = run_cli(&[
        "replay",
        "--wal",
        session.wal.to_str().unwrap(),
        "--snapshot",
        session.snapshot.to_str().unwrap(),
        "--max-orders",
        "64",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let _ = fs::remove_dir_all(&session.dir);
}

fn bench_args(ops: &str, seed: &str) -> Vec<String> {
    vec![
        "bench".to_string(),
        "--ops".to_string(),
        ops.to_string(),
        "--seed".to_string(),
        seed.to_string(),
    ]
}

fn bench_refs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

fn field(text: &str, name: &str) -> u64 {
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=')
            && key == name
        {
            return value.parse().expect("numeric field");
        }
    }
    panic!("field {name} missing from: {text}");
}

#[test]
fn bench_is_deterministic_and_consistent() {
    let args = bench_args("3000", "42");
    let first = run_cli(&bench_refs(&args));
    let second = run_cli(&bench_refs(&args));
    assert!(first.status.success());
    let text = stdout_of(&first);
    assert_eq!(text, stdout_of(&second), "same seed, same output");
    assert!(text.starts_with("seed=42\n"), "text was: {text}");
    assert_eq!(field(&text, "ops"), 3000);
    assert_eq!(field(&text, "applied") + field(&text, "rejected"), 3000);
    assert!(field(&text, "trades") >= 1, "the load must trade: {text}");
}

#[test]
fn bench_json_shape() {
    let mut args = bench_args("500", "42");
    args.push("--json".to_string());
    let output = run_cli(&bench_refs(&args));
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("bench --json prints JSON");
    assert_eq!(value["seed"], 42);
    assert_eq!(value["ops"], 500);
    for key in [
        "applied",
        "rejected",
        "trades",
        "live_orders",
        "best_bid",
        "best_ask",
    ] {
        assert!(value.get(key).is_some(), "missing key {key}");
    }
    assert_eq!(
        value["applied"].as_u64().unwrap() + value["rejected"].as_u64().unwrap(),
        500
    );
}

#[test]
fn bench_seed_comes_from_sim_seed_and_flag_wins() {
    let from_env = run_cli_env(&["bench", "--ops", "10"], &[("SIM_SEED", "7")]);
    assert!(stdout_of(&from_env).starts_with("seed=7\n"));
    let from_flag = run_cli_env(
        &["bench", "--ops", "10", "--seed", "9"],
        &[("SIM_SEED", "7")],
    );
    assert!(stdout_of(&from_flag).starts_with("seed=9\n"));
}
