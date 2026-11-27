//! Behaviour tests for the chronicle-cli binary (ARCHITECTURE.md
//! section 2.3). The fixture drives a live book and a WAL writer in
//! lockstep, so the expected event stream is the engine's own output
//! formatted by the contract formatter in this file; the binary must
//! reproduce it from the files alone.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use chronicle_core::{
    Book, BookConfig, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, SnapshotState, StpPolicy,
};
use chronicle_wal::{Crc32c, SyncPolicy, WalCommand, WalWriter, load_snapshot, save_snapshot};

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
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
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
    let mut args = vec![
        "replay",
        "--wal",
        session.wal.to_str().unwrap(),
        "--seed",
        "42",
    ];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected = String::from("seed=42\n");
    expected.push_str(&joined(&session.lines));
    assert_eq!(stdout_of(&output), expected);
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
        "--seed",
        "42",
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
    let mut expected = String::from("seed=42\n");
    expected.push_str(&joined(&later));
    assert_eq!(stdout_of(&output), expected);
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn det_cli_replay_twice_byte_identical() {
    // INV-DET-1: the same command sequence at the same seed
    // produces byte-identical replay output across separate OS
    // processes. Each run_cli call spawns its own process, and the
    // full stdout bytes are compared, banner included.
    let session = build_session("det-twice");
    let mut args = vec![
        "replay",
        "--wal",
        session.wal.to_str().unwrap(),
        "--seed",
        "42",
    ];
    args.extend(CONFIG_FLAGS);
    let first = run_cli(&args);
    let second = run_cli(&args);
    assert!(
        first.status.success() && second.status.success(),
        "stderr: {} / {}",
        String::from_utf8_lossy(&first.stderr),
        String::from_utf8_lossy(&second.stderr)
    );
    let mut expected = String::from("seed=42\n");
    expected.push_str(&joined(&session.lines));
    assert_eq!(stdout_of(&first), expected);
    assert_eq!(
        first.stdout, second.stdout,
        "replay output must be byte-identical across processes"
    );
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn seed_banner_printed() {
    // INV-DET-2: the CLI bench and replay commands emit the seed
    // line at startup, from the flag or from SIM_SEED.
    let session = build_session("seed-banner");
    let mut args = vec![
        "replay",
        "--wal",
        session.wal.to_str().unwrap(),
        "--seed",
        "42",
    ];
    args.extend(CONFIG_FLAGS);
    let replay = run_cli(&args);
    assert!(replay.status.success());
    assert!(
        stdout_of(&replay).starts_with("seed=42\n"),
        "replay stdout was: {}",
        stdout_of(&replay)
    );
    let bench = run_cli(&["bench", "--ops", "10", "--seed", "42"]);
    assert!(stdout_of(&bench).starts_with("seed=42\n"));
    let mut env_args = vec!["replay", "--wal", session.wal.to_str().unwrap()];
    env_args.extend(CONFIG_FLAGS);
    let from_env = run_cli_env(&env_args, &[("SIM_SEED", "7")]);
    assert!(from_env.status.success());
    assert!(
        stdout_of(&from_env).starts_with("seed=7\n"),
        "replay stdout was: {}",
        stdout_of(&from_env)
    );
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
fn checkpoint_writes_a_verified_snapshot_and_archives_the_log() {
    // Lane T5 (ARCHITECTURE.md section 10.6): checkpoint recovers
    // the session log, writes the snapshot, verifies it by loading
    // it back, and only then archives the log under the covered
    // sequence. Recovery continues from the pair.
    let session = build_session("checkpoint");
    let wal_bytes = fs::read(&session.wal).expect("read wal");
    let out_path = session.dir.join("checkpoint.snapshot");
    let archive = session.dir.join("book.wal.archive-5");
    let mut args = vec![
        "checkpoint",
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
        format!(
            "checkpoint: written last_included_seq=5 live_orders=1 archived_log={}\n",
            archive.display()
        )
    );
    assert!(!session.wal.exists(), "the live log moved to the archive");
    assert_eq!(
        fs::read(&archive).expect("read archive"),
        wal_bytes,
        "the archive holds the log byte for byte"
    );
    let loaded = load_snapshot(&out_path, &config()).expect("checkpoint snapshot loads");
    assert_eq!(loaded, session.final_state);

    // The pair verifies: the snapshot covers the archived log in
    // full, and recovery continues from exactly this state.
    let verify_args = vec![
        "verify",
        "--wal",
        archive.to_str().unwrap(),
        "--snapshot",
        out_path.to_str().unwrap(),
    ];
    let output = run_cli(&verify_args);
    assert!(output.status.success());
    assert_eq!(
        stdout_of(&output),
        "snapshot: ok last_included_seq=5 live_orders=1\nwal: ok frames=5 discarded=0 last_seq=5\nverdict: ok\n"
    );
    let _ = fs::remove_dir_all(&session.dir);
}

#[test]
fn checkpoint_of_a_missing_log_is_a_usage_error() {
    let dir = test_dir("checkpoint-missing");
    let missing = dir.join("nope.wal");
    let out_path = dir.join("checkpoint.snapshot");
    let mut args = vec![
        "checkpoint",
        "--wal",
        missing.to_str().unwrap(),
        "--out",
        out_path.to_str().unwrap(),
    ];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("error: cannot read log"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out_path.exists(), "no snapshot is written on failure");
    let _ = fs::remove_dir_all(&dir);
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

#[test]
fn verify_and_replay_stream_a_million_frame_log() {
    // Lane S5: verify and replay read the log through the
    // streaming scanner, so a log far larger than the fixture
    // session runs in bounded memory with byte-identical output.
    let dir = test_dir("million-frames");
    let wal = dir.join("million.wal");
    let file = fs::File::create(&wal).expect("create wal");
    let mut writer = WalWriter::new(file, SyncPolicy::Never);
    for seq in 1..=1_000_000_u64 {
        writer
            .append(&add(seq, 1, Side::Bid, 100, 1), seq)
            .expect("append");
    }
    drop(writer);

    let mut args = vec!["verify", "--wal", wal.to_str().unwrap()];
    args.extend(CONFIG_FLAGS);
    let output = run_cli(&args);
    assert!(output.status.success());
    assert_eq!(
        stdout_of(&output),
        "wal: ok frames=1000000 discarded=0 last_seq=1000000\nverdict: ok\n"
    );

    let output = run_cli(&[
        "replay",
        "--wal",
        wal.to_str().unwrap(),
        "--seed",
        "0",
        "--max-orders",
        "1000000",
        "--grid-len",
        "640",
        "--tick-size",
        "1",
        "--grid-start-tick",
        "1",
    ]);
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("seed=0"));
    assert_eq!(lines.next(), Some("1 accepted order=1"));
    assert_eq!(stdout.lines().count(), 1_000_001);
    assert_eq!(
        stdout.lines().last(),
        Some("1000000 accepted order=1000000")
    );

    // Lane T1: the snapshot subcommand recovers through the
    // streaming form as well, over the same million-frame log.
    let snapshot_out = dir.join("million.snapshot");
    let output = run_cli(&[
        "snapshot",
        "--wal",
        wal.to_str().unwrap(),
        "--out",
        snapshot_out.to_str().unwrap(),
        "--max-orders",
        "1000000",
        "--grid-len",
        "640",
        "--tick-size",
        "1",
        "--grid-start-tick",
        "1",
    ]);
    assert!(output.status.success());
    assert_eq!(
        stdout_of(&output),
        "snapshot: written last_included_seq=1000000 live_orders=1000000\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn write_wal_cycle(wal: &std::path::Path, book: &mut Book, cmds: &[WalCommand]) -> SnapshotState {
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let file = fs::File::create(wal).expect("create wal cycle");
    let mut writer = WalWriter::new(file, SyncPolicy::Never);
    for cmd in cmds {
        let seq = book.next_seq();
        let mut events = EventBuffer::new(&mut storage);
        cmd.apply(book, &mut events).expect("apply");
        storage.clear();
        writer.append(cmd, seq).expect("append");
    }
    book.capture_snapshot_state()
}

#[test]
fn multi_cycle_checkpoint_and_snapshot_across_seams() {
    let dir = test_dir("multi-checkpoint");
    let wal = dir.join("book.wal");
    let snap = dir.join("book.snap");
    let preview_snap = dir.join("preview.snap");
    let (wal_s, snap_s, preview_s) = (
        wal.to_str().unwrap(),
        snap.to_str().unwrap(),
        preview_snap.to_str().unwrap(),
    );
    let mut live_book = Book::new(config()).expect("book");

    // Cycle 1: seqs 1..=3 into book.wal, then Checkpoint #1.
    let state_after_3 = write_wal_cycle(
        &wal,
        &mut live_book,
        &[
            add(1, 1, Side::Bid, 100, 10),
            add(2, 2, Side::Ask, 102, 6),
            add(3, 1, Side::Bid, 99, 5),
        ],
    );
    let mut args1 = vec!["checkpoint", "--wal", wal_s, "--out", snap_s];
    args1.extend(CONFIG_FLAGS);
    let out1 = run_cli(&args1);
    assert!(
        out1.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(!wal.exists() && dir.join("book.wal.archive-3").exists());
    assert_eq!(load_snapshot(&snap, &config()).expect("#1"), state_after_3);

    // Cycle 2: fresh book.wal starting at seq 4..=6.
    let state_after_6 = write_wal_cycle(
        &wal,
        &mut live_book,
        &[
            add(4, 2, Side::Ask, 100, 4),
            WalCommand::Amend {
                order_id: OrderId(3),
                new_price: 98,
                new_qty: 8,
            },
            WalCommand::Cancel {
                order_id: OrderId(2),
            },
        ],
    );
    let snap_out = run_cli(&[
        "snapshot",
        "--wal",
        wal_s,
        "--snapshot",
        snap_s,
        "--out",
        preview_s,
    ]);
    assert!(snap_out.status.success());
    assert_eq!(
        stdout_of(&snap_out),
        "snapshot: written last_included_seq=6 live_orders=2\n"
    );
    assert_eq!(
        load_snapshot(&preview_snap, &config()).expect("preview"),
        state_after_6
    );

    // Checkpoint #2 with in-place `--snapshot book.snap --out book.snap`.
    let out2 = run_cli(&[
        "checkpoint",
        "--wal",
        wal_s,
        "--snapshot",
        snap_s,
        "--out",
        snap_s,
    ]);
    assert!(out2.status.success());
    let archive2 = dir.join("book.wal.archive-6");
    assert_eq!(
        stdout_of(&out2),
        format!(
            "checkpoint: written last_included_seq=6 live_orders=2 archived_log={}\n",
            archive2.display()
        )
    );
    assert!(!wal.exists() && archive2.exists());
    assert_eq!(load_snapshot(&snap, &config()).expect("#2"), state_after_6);

    // Cycle 3: fresh WAL starting at seq 7 replays cleanly from Checkpoint #2.
    write_wal_cycle(&wal, &mut live_book, &[add(7, 3, Side::Ask, 105, 3)]);
    let replay_out = run_cli(&[
        "replay",
        "--wal",
        wal_s,
        "--snapshot",
        snap_s,
        "--seed",
        "77",
    ]);
    assert!(replay_out.status.success());
    assert_eq!(stdout_of(&replay_out), "seed=77\n7 accepted order=7\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_rejects_snapshot_with_zero_max_orders() {
    let dir = test_dir("verify-zero-max-orders");
    let wal = dir.join("empty.wal");
    fs::write(&wal, []).expect("write empty wal");
    let snap = dir.join("zero_max_orders.snap");

    let grid_len = 64u32;
    let total_len = 68usize + 20 * (grid_len as usize);
    let mut bytes = vec![0u8; total_len];
    bytes[0..4].copy_from_slice(b"CSNP");
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&64u16.to_le_bytes());
    bytes[8..16].copy_from_slice(&0u64.to_le_bytes());
    bytes[16..24].copy_from_slice(&1u64.to_le_bytes());
    bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&0u32.to_le_bytes()); // max_orders = 0
    bytes[32..40].copy_from_slice(&1u64.to_le_bytes());
    bytes[40..48].copy_from_slice(&1u64.to_le_bytes());
    bytes[48..52].copy_from_slice(&grid_len.to_le_bytes());
    bytes[52..56].copy_from_slice(&0u32.to_le_bytes());
    bytes[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
    for i in 0..(grid_len as usize) {
        let at = 64 + i * 20;
        bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    let body_end = total_len - 4;
    let mut crc = Crc32c::new();
    crc.update(b"chronicle-snapshot/v1");
    crc.update(&[0]);
    crc.update(&bytes[..body_end]);
    bytes[body_end..].copy_from_slice(&crc.finalize().to_le_bytes());
    fs::write(&snap, &bytes).expect("write malformed snapshot");

    let output = run_cli(&[
        "verify",
        "--wal",
        wal.to_str().unwrap(),
        "--snapshot",
        snap.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout_of(&output).starts_with("verdict: corrupt"),
        "stdout was: {}",
        stdout_of(&output)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn replay_exits_cleanly_on_broken_pipe() {
    let dir = test_dir("broken-pipe");
    let wal = dir.join("pipe.wal");
    {
        let file = fs::File::create(&wal).expect("create wal");
        let mut writer = WalWriter::new(file, SyncPolicy::Never);
        for seq in 1..=2_000_u64 {
            writer
                .append(&add(seq, 1, Side::Bid, 100, 1), seq)
                .expect("append");
        }
    }
    let mut child = Command::new(BIN)
        .args([
            "replay",
            "--wal",
            wal.to_str().unwrap(),
            "--seed",
            "0",
            "--max-orders",
            "4096",
            "--grid-len",
            "640",
            "--tick-size",
            "1",
            "--grid-start-tick",
            "1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn replay");
    {
        let child_out = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(child_out);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read seed line");
        assert_eq!(line, "seed=0\n");
        // Drop `reader` (closing the read end of the pipe) while the
        // child still has ~48 KiB of buffered events to flush.
    }
    let output = child.wait_with_output().expect("wait child");
    assert!(
        output.status.success(),
        "expected exit 0 on BrokenPipe, got {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, b"");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn completions_supports_all_shells_and_covers_parser_surface() {
    use chronicle_lob_cli::{Shell, completions_script, parser, write_completions};

    assert_eq!("bash".parse::<Shell>(), Ok(Shell::Bash));
    assert_eq!("zsh".parse::<Shell>(), Ok(Shell::Zsh));
    assert_eq!("fish".parse::<Shell>(), Ok(Shell::Fish));
    assert_eq!("powershell".parse::<Shell>(), Ok(Shell::PowerShell));
    assert_eq!("pwsh".parse::<Shell>(), Ok(Shell::PowerShell));
    assert_eq!("PWSH".parse::<Shell>(), Ok(Shell::PowerShell));
    assert!("tcsh".parse::<Shell>().is_err());

    assert_eq!(Shell::Bash.to_string(), "bash");
    assert_eq!(Shell::Zsh.to_string(), "zsh");
    assert_eq!(Shell::Fish.to_string(), "fish");
    assert_eq!(Shell::PowerShell.to_string(), "powershell");

    let cmd = parser();
    let subcommands: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    assert_eq!(
        subcommands,
        vec![
            "replay",
            "verify",
            "snapshot",
            "checkpoint",
            "bench",
            "completions"
        ]
    );

    let mut long_flags: Vec<String> = cmd
        .get_arguments()
        .filter_map(|arg| arg.get_long().map(str::to_string))
        .collect();
    for sub in cmd.get_subcommands() {
        for arg in sub.get_arguments() {
            if let Some(long) = arg.get_long()
                && !long_flags.iter().any(|existing| existing == long)
            {
                long_flags.push(long.to_string());
            }
        }
    }
    for builtin in ["help", "version"] {
        if !long_flags.iter().any(|existing| existing == builtin) {
            long_flags.push(builtin.to_string());
        }
    }

    for (name, shell) in [
        ("bash", Shell::Bash),
        ("zsh", Shell::Zsh),
        ("fish", Shell::Fish),
        ("powershell", Shell::PowerShell),
        ("pwsh", Shell::PowerShell),
    ] {
        let output = run_cli(&["completions", name]);
        assert!(
            output.status.success(),
            "completions {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = stdout_of(&output);
        assert_eq!(text, completions_script(shell));

        let mut buf = Vec::new();
        write_completions(shell, &mut buf).expect("write_completions");
        assert_eq!(buf, output.stdout);

        for sub in &subcommands {
            assert!(
                text.contains(sub.as_str()),
                "{name} completions missing subcommand {sub}"
            );
        }
        for flag in &long_flags {
            let needle = if shell == Shell::Fish {
                format!("-l {flag}")
            } else {
                format!("--{flag}")
            };
            assert!(
                text.contains(&needle),
                "{name} completions missing flag {needle}"
            );
        }
    }
}

#[test]
fn completions_rejects_invalid_or_missing_shell_with_exit_2() {
    let invalid = run_cli(&["completions", "tcsh"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(invalid.stdout, b"");

    let missing = run_cli(&["completions"]);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(missing.stdout, b"");
}
