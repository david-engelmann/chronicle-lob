//! chronicle-cli, the command line tool for chronicle books
//! (ARCHITECTURE.md section 2.3). One binary with five subcommands:
//! replay applies a write-ahead log to a book and prints the event
//! stream, verify reports the sequence and checksum health of a log
//! and snapshot pair, snapshot writes a snapshot from a log,
//! checkpoint writes a verified snapshot and archives the log it
//! covers, and bench runs a seeded synthetic load. This is the only
//! crate in the workspace that parses text or prints JSON. Output
//! ordering is fully determined by the input files and the seed.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chronicle_core::{
    Book, BookConfig, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, SnapshotState, StpPolicy,
};
use chronicle_wal::{
    ScannedFrame, WalError, decode_snapshot, load_snapshot, recover_from_reader, save_snapshot,
    scan_reader_with, snapshot_config,
};
use clap::{Arg, ArgMatches, Command};

/// Build the argument parser. Every subcommand accepts `--seed`,
/// which honours the `SIM_SEED` environment variable, per section 2.3.
fn parser() -> Command {
    let seed = Arg::new("seed")
        .long("seed")
        .value_name("U64")
        .value_parser(clap::value_parser!(u64))
        .env("SIM_SEED")
        .default_value("0")
        .global(true)
        .help("Seed for seeded work; honours SIM_SEED");
    Command::new("chronicle-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Replay, verify, snapshot, checkpoint, and benchmark chronicle books")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(seed)
        .subcommand(
            Command::new("replay")
                .about("Apply a write-ahead log and print the event stream")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .args(config_args()),
        )
        .subcommand(
            Command::new("verify")
                .about("Report the sequence and checksum health of a log and snapshot pair")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .args(config_args()),
        )
        .subcommand(
            Command::new("snapshot")
                .about("Write a snapshot from a write-ahead log and optional base snapshot")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .arg(out_arg())
                .args(config_args()),
        )
        .subcommand(
            Command::new("checkpoint")
                .about("Write a verified snapshot and archive the log it covers")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .arg(out_arg())
                .args(config_args()),
        )
        .subcommand(
            Command::new("bench")
                .about("Run a seeded synthetic load against a fresh book")
                .arg(
                    Arg::new("ops")
                        .long("ops")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("100000")
                        .help("Number of generated commands to apply"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(clap::ArgAction::SetTrue)
                        .help("Print the summary as JSON"),
                ),
        )
}

fn wal_arg() -> Arg {
    Arg::new("wal")
        .long("wal")
        .value_name("PATH")
        .value_parser(clap::value_parser!(PathBuf))
        .required(true)
        .help("Write-ahead log file")
}

fn snapshot_arg() -> Arg {
    Arg::new("snapshot")
        .long("snapshot")
        .value_name("PATH")
        .value_parser(clap::value_parser!(PathBuf))
        .help("Snapshot file supplying the base book and its configuration")
}

fn out_arg() -> Arg {
    Arg::new("out")
        .long("out")
        .value_name("PATH")
        .value_parser(clap::value_parser!(PathBuf))
        .required(true)
        .help("Snapshot file to write")
}

/// The book configuration flags. When `--snapshot` is supplied, the
/// snapshot header carries the configuration instead: the flags
/// conflict with `--snapshot` and are required without one.
fn config_args() -> Vec<Arg> {
    let max_orders = Arg::new("max-orders")
        .long("max-orders")
        .value_name("U32")
        .value_parser(clap::value_parser!(u32))
        .required_unless_present("snapshot")
        .conflicts_with("snapshot")
        .help("Arena capacity of the book");
    let grid_len = Arg::new("grid-len")
        .long("grid-len")
        .value_name("U32")
        .value_parser(clap::value_parser!(u32))
        .required_unless_present("snapshot")
        .conflicts_with("snapshot")
        .help("Number of price ticks on the grid");
    let tick_size = Arg::new("tick-size")
        .long("tick-size")
        .value_name("U64")
        .value_parser(clap::value_parser!(u64))
        .required_unless_present("snapshot")
        .conflicts_with("snapshot")
        .help("Quote units per tick");
    let grid_start_tick = Arg::new("grid-start-tick")
        .long("grid-start-tick")
        .value_name("U64")
        .value_parser(clap::value_parser!(u64))
        .required_unless_present("snapshot")
        .conflicts_with("snapshot")
        .help("Tick value of the first grid level");
    let default_stp = Arg::new("default-stp")
        .long("default-stp")
        .value_name("POLICY")
        .value_parser(["cancel-incoming", "cancel-resting", "cancel-both"])
        .conflicts_with("snapshot")
        .help("Default self-trade prevention policy");
    vec![
        max_orders,
        grid_len,
        tick_size,
        grid_start_tick,
        default_stp,
    ]
}

/// The configuration named by the flags. The parser guarantees the
/// four numeric flags are present whenever this runs.
fn config_from_flags(sub: &ArgMatches) -> BookConfig {
    BookConfig {
        max_orders: *sub
            .get_one::<u32>("max-orders")
            .expect("the parser requires max-orders here"),
        grid_len: *sub
            .get_one::<u32>("grid-len")
            .expect("the parser requires grid-len here"),
        tick_size: *sub
            .get_one::<u64>("tick-size")
            .expect("the parser requires tick-size here"),
        grid_start_tick: *sub
            .get_one::<u64>("grid-start-tick")
            .expect("the parser requires grid-start-tick here"),
        default_stp: match sub.get_one::<String>("default-stp").map(String::as_str) {
            Some("cancel-resting") => StpPolicy::CancelResting,
            Some("cancel-both") => StpPolicy::CancelBoth,
            _ => StpPolicy::CancelIncoming,
        },
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::from(2)
}

fn corrupt(error: &WalError) -> ExitCode {
    println!("verdict: corrupt {error}");
    ExitCode::from(1)
}

const fn reason_token(reason: CancelReason) -> &'static str {
    match reason {
        CancelReason::Requested => "requested",
        CancelReason::ByParticipant => "by-participant",
        CancelReason::SelfTradePrevention => "self-trade-prevention",
    }
}

/// Write the section 2.3 event line directly into `out` without
/// allocating an intermediate `String` per event.
fn write_event(out: &mut impl Write, event: &ExecutionEvent) -> io::Result<()> {
    match event {
        ExecutionEvent::Accepted { seq, order_id } => {
            writeln!(out, "{seq} accepted order={}", order_id.0)
        }
        ExecutionEvent::Trade {
            seq,
            maker_order_id,
            taker_order_id,
            price,
            qty,
        } => writeln!(
            out,
            "{seq} trade maker={} taker={} price={price} qty={qty}",
            maker_order_id.0, taker_order_id.0
        ),
        ExecutionEvent::Filled { seq, order_id } => {
            writeln!(out, "{seq} filled order={}", order_id.0)
        }
        ExecutionEvent::Cancelled {
            seq,
            order_id,
            reason,
        } => writeln!(
            out,
            "{seq} cancelled order={} reason={}",
            order_id.0,
            reason_token(*reason)
        ),
        ExecutionEvent::Amended {
            seq,
            order_id,
            price,
            qty,
        } => writeln!(
            out,
            "{seq} amended order={} price={price} qty={qty}",
            order_id.0
        ),
        ExecutionEvent::Expired {
            seq,
            order_id,
            remaining_qty,
        } => writeln!(
            out,
            "{seq} expired order={} remaining={remaining_qty}",
            order_id.0
        ),
        ExecutionEvent::SelfTradePrevented { seq, order_id } => {
            writeln!(out, "{seq} self-trade-prevented order={}", order_id.0)
        }
    }
}

/// Streaming sequence continuity check shared by replay and verify:
/// frames at or below `base_seq` are covered by the base snapshot,
/// and the rest must step by exactly one from `base_seq + 1` using
/// checked arithmetic.
struct SequenceChecker {
    base_seq: u64,
    expected: Option<u64>,
}

impl SequenceChecker {
    const fn new(base_seq: u64) -> Self {
        Self {
            base_seq,
            expected: base_seq.checked_add(1),
        }
    }

    const fn check(&mut self, frame: &ScannedFrame) -> Result<(), WalError> {
        if frame.seq <= self.base_seq {
            return Ok(());
        }
        let Some(expected) = self.expected else {
            return Err(WalError::WalSequenceGap {
                expected: u64::MAX,
                found: frame.seq,
                offset: frame.offset,
            });
        };
        if frame.seq != expected {
            return Err(WalError::WalSequenceGap {
                expected,
                found: frame.seq,
                offset: frame.offset,
            });
        }
        self.expected = expected.checked_add(1);
        Ok(())
    }
}

struct Inputs {
    wal_path: PathBuf,
    config: BookConfig,
    base_snapshot: Option<SnapshotState>,
}

/// Resolve the inputs of a WAL subcommand: the log path, the book
/// configuration, and the decoded base snapshot when `--snapshot` is
/// supplied, reading the snapshot file from disk only once.
fn inputs(sub: &ArgMatches) -> Result<Inputs, String> {
    let wal_path = sub
        .get_one::<PathBuf>("wal")
        .expect("the parser requires --wal")
        .clone();
    let (config, base_snapshot) = match sub.get_one::<PathBuf>("snapshot") {
        Some(path) => {
            let bytes = fs::read(path)
                .map_err(|e| format!("cannot read snapshot {}: {e}", path.display()))?;
            let config = snapshot_config(&bytes).map_err(|e| e.to_string())?;
            let state = decode_snapshot(&bytes, &config).map_err(|e| e.to_string())?;
            (config, Some(state))
        }
        None => (config_from_flags(sub), None),
    };
    Ok(Inputs {
        wal_path,
        config,
        base_snapshot,
    })
}

fn replay(sub: &ArgMatches) -> Result<(), String> {
    let seed = *sub.get_one::<u64>("seed").expect("seed has a default");
    let inputs = inputs(sub)?;
    let (mut book, base_seq) = match inputs.base_snapshot {
        Some(state) => {
            let base_seq = state
                .next_seq
                .checked_sub(1)
                .ok_or_else(|| "snapshot next_seq must be at least 1".to_string())?;
            let book = Book::from_snapshot_state(state).map_err(|e| e.to_string())?;
            (book, base_seq)
        }
        None => (Book::new(inputs.config).map_err(|e| e.to_string())?, 0),
    };
    let file = fs::File::open(&inputs.wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", inputs.wal_path.display()))?;
    // INV-DET-2: the seed banner prints at startup, before any
    // event line, buffered alongside the event stream.
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    if let Err(err) = writeln!(out, "seed={seed}") {
        return if err.kind() == io::ErrorKind::BrokenPipe {
            Ok(())
        } else {
            Err(format!("cannot write replay output: {err}"))
        };
    }
    let mut storage: Vec<ExecutionEvent> =
        Vec::with_capacity(2 * inputs.config.max_orders as usize + 2);
    let mut seq_check = SequenceChecker::new(base_seq);
    let mut frame_rejection: Option<String> = None;
    let scan_res = scan_reader_with(file, |frame| {
        seq_check.check(&frame)?;
        if frame.seq <= base_seq {
            return Ok(());
        }
        let written = {
            let mut events = EventBuffer::new(&mut storage);
            match frame.command.apply(&mut book, &mut events) {
                Ok(written) => written,
                Err(source) => {
                    frame_rejection = Some(format!(
                        "frame at offset {} (seq {}) rejected: {source}",
                        frame.offset, frame.seq
                    ));
                    return Err(WalError::Book {
                        seq: Some(frame.seq),
                        source,
                    });
                }
            }
        };
        for event in &storage[..written] {
            write_event(&mut out, event).map_err(|source| WalError::Io {
                operation: "write replay output",
                source,
            })?;
        }
        storage.clear();
        Ok(())
    });
    match scan_res {
        Ok(_) => match out.flush() {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(err) => Err(format!("cannot flush replay output: {err}")),
        },
        Err(WalError::Io {
            operation: "write replay output",
            source,
        }) if source.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(frame_rejection.unwrap_or_else(|| err.to_string())),
    }
}

fn run_replay(sub: &ArgMatches) -> ExitCode {
    match replay(sub) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

fn verify(sub: &ArgMatches) -> ExitCode {
    let wal_path = sub
        .get_one::<PathBuf>("wal")
        .expect("the parser requires --wal")
        .clone();
    let mut base_seq = 0u64;
    if let Some(path) = sub.get_one::<PathBuf>("snapshot") {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => return fail(&format!("cannot read snapshot {}: {e}", path.display())),
        };
        let config = match snapshot_config(&bytes) {
            Ok(config) => config,
            Err(e) => return corrupt(&e),
        };
        match decode_snapshot(&bytes, &config) {
            Ok(state) => {
                base_seq = state.next_seq.saturating_sub(1);
                println!(
                    "snapshot: ok last_included_seq={} live_orders={}",
                    base_seq,
                    state.live_orders.len()
                );
            }
            Err(e) => return corrupt(&e),
        }
    } else if let Err(e) = config_from_flags(sub).validate() {
        return fail(&e.to_string());
    }
    let file = match fs::File::open(&wal_path) {
        Ok(file) => file,
        Err(e) => return fail(&format!("cannot read log {}: {e}", wal_path.display())),
    };
    let mut seq_check = SequenceChecker::new(base_seq);
    match scan_reader_with(file, |frame| seq_check.check(&frame)) {
        Ok(report) => {
            println!(
                "wal: ok frames={} discarded={} last_seq={}",
                report.frame_count,
                report.discarded_trailing_bytes,
                report.last_seq.unwrap_or(0)
            );
            println!("verdict: ok");
            ExitCode::SUCCESS
        }
        Err(e) => corrupt(&e),
    }
}

fn snapshot_command(sub: &ArgMatches) -> Result<(), String> {
    let inputs = inputs(sub)?;
    let out_path = sub
        .get_one::<PathBuf>("out")
        .expect("the parser requires --out")
        .clone();
    let file = fs::File::open(&inputs.wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", inputs.wal_path.display()))?;
    let recovery = recover_from_reader(inputs.config, inputs.base_snapshot, file)
        .map_err(|e| e.to_string())?;
    let state = recovery.book.capture_snapshot_state();
    save_snapshot(&out_path, &state).map_err(|e| e.to_string())?;
    println!(
        "snapshot: written last_included_seq={} live_orders={}",
        state.next_seq.saturating_sub(1),
        state.live_orders.len()
    );
    Ok(())
}

fn run_snapshot(sub: &ArgMatches) -> ExitCode {
    match snapshot_command(sub) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

/// The archive path for a checkpointed log: the log's own path
/// with the covered sequence appended, so successive checkpoints
/// of one log archive under distinct names.
fn archive_path_for(wal_path: &Path, last_included_seq: u64) -> PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(format!(".archive-{last_included_seq}"));
    PathBuf::from(name)
}

fn checkpoint_command(sub: &ArgMatches) -> Result<(), String> {
    // `inputs` reads and decodes the optional `--snapshot` completely
    // in memory before we open the WAL or write `--out`, so `--snapshot
    // foo.snap --out foo.snap` updates in place safely through
    // `save_snapshot`'s atomic `.tmp` write, fsync, and rename.
    let inputs = inputs(sub)?;
    let out_path = sub
        .get_one::<PathBuf>("out")
        .expect("the parser requires --out")
        .clone();
    let file = fs::File::open(&inputs.wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", inputs.wal_path.display()))?;
    let recovery = recover_from_reader(inputs.config, inputs.base_snapshot, file)
        .map_err(|e| e.to_string())?;
    let state = recovery.book.capture_snapshot_state();
    let last_included_seq = state.next_seq.saturating_sub(1);
    save_snapshot(&out_path, &state).map_err(|e| e.to_string())?;
    // Verify before archiving (ARCHITECTURE.md section 10.6): the
    // snapshot on disk must load and name the sequence the log
    // reached. Until this passes, the log stays where it is.
    let loaded = load_snapshot(&out_path, &inputs.config).map_err(|e| e.to_string())?;
    if loaded.next_seq != state.next_seq {
        return Err(format!(
            "checkpoint verification failed: the snapshot names next_seq {} but the log reached {}",
            loaded.next_seq, state.next_seq
        ));
    }
    let archive_path = archive_path_for(&inputs.wal_path, last_included_seq);
    if archive_path.exists() {
        fs::remove_file(&archive_path).map_err(|e| {
            format!(
                "cannot replace archived log {}: {e}",
                archive_path.display()
            )
        })?;
    }
    fs::rename(&inputs.wal_path, &archive_path)
        .map_err(|e| format!("cannot archive log {}: {e}", inputs.wal_path.display()))?;
    println!(
        "checkpoint: written last_included_seq={} live_orders={} archived_log={}",
        last_included_seq,
        state.live_orders.len(),
        archive_path.display()
    );
    Ok(())
}

fn run_checkpoint(sub: &ArgMatches) -> ExitCode {
    match checkpoint_command(sub) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

/// `SplitMix64`, the workspace's deterministic generator (section 11).
struct SplitMix64(u64);

impl SplitMix64 {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The fixed bench book: large enough that the generated id space
/// keeps thousands of orders live.
const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 65_536,
        grid_len: 6400,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

struct BenchSummary {
    applied: u64,
    rejected: u64,
    trades: u64,
    live_orders: u32,
    best_bid: Option<u64>,
    best_ask: Option<u64>,
}

/// The seeded synthetic load: a deterministic mix of adds of every
/// order type, cancels, amends, mass cancels, and zero-quantity
/// rejections over a 5,000-id space, driven by `SplitMix64`.
fn bench(seed: u64, ops: u64) -> Result<BenchSummary, String> {
    let mut book = Book::new(bench_config()).map_err(|e| e.to_string())?;
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(32_768);
    let mut rng = SplitMix64(seed);
    let mut summary = BenchSummary {
        applied: 0,
        rejected: 0,
        trades: 0,
        live_orders: 0,
        best_bid: None,
        best_ask: None,
    };
    for _ in 0..ops {
        let r = rng.next();
        let slot = r % 100;
        let participant = ParticipantId(u32::try_from((r >> 24) % 8).expect("mod 8 fits u32"));
        let order_id = OrderId(1 + (r >> 32) % 5000);
        let side = if (r >> 8).is_multiple_of(2) {
            Side::Bid
        } else {
            Side::Ask
        };
        let price = 250 + (r >> 9) % 100;
        let qty = 1 + (r >> 17) % 25;
        let mut events = EventBuffer::new(&mut storage);
        let result = if slot < 70 {
            let order_type = match slot {
                0..=54 => OrderType::Limit,
                55..=64 => OrderType::Ioc,
                _ if (r >> 11).is_multiple_of(2) => OrderType::Fok,
                _ => OrderType::PostOnly,
            };
            book.add_order(
                order_id,
                participant,
                side,
                order_type,
                price,
                qty,
                None,
                &mut events,
            )
        } else if slot < 85 {
            book.cancel_order(order_id, &mut events)
        } else if slot < 90 {
            book.amend_order(order_id, price, qty, &mut events)
        } else if slot < 95 {
            book.cancel_by_participant(participant, &mut events)
        } else {
            book.add_order(
                order_id,
                participant,
                Side::Bid,
                OrderType::Limit,
                100,
                0,
                None,
                &mut events,
            )
        };
        match result {
            Ok(written) => {
                summary.applied += 1;
                summary.trades += storage[..written]
                    .iter()
                    .filter(|event| matches!(event, ExecutionEvent::Trade { .. }))
                    .count() as u64;
            }
            Err(_) => summary.rejected += 1,
        }
        storage.clear();
    }
    summary.live_orders = book.live_order_count();
    summary.best_bid = book.best_bid().map(|(price, _)| price);
    summary.best_ask = book.best_ask().map(|(price, _)| price);
    Ok(summary)
}

fn price_or_none(price: Option<u64>) -> String {
    price.map_or_else(|| "none".to_string(), |price| price.to_string())
}

fn print_bench(seed: u64, ops: u64, summary: &BenchSummary, json: bool) {
    if json {
        let value = serde_json::json!({
            "seed": seed,
            "ops": ops,
            "applied": summary.applied,
            "rejected": summary.rejected,
            "trades": summary.trades,
            "live_orders": summary.live_orders,
            "best_bid": summary.best_bid,
            "best_ask": summary.best_ask,
        });
        println!("{value}");
    } else {
        println!("seed={seed}");
        println!("ops={ops}");
        println!("applied={}", summary.applied);
        println!("rejected={}", summary.rejected);
        println!("trades={}", summary.trades);
        println!("live_orders={}", summary.live_orders);
        println!("best_bid={}", price_or_none(summary.best_bid));
        println!("best_ask={}", price_or_none(summary.best_ask));
    }
}

fn run_bench(sub: &ArgMatches) -> ExitCode {
    let seed = *sub.get_one::<u64>("seed").expect("seed has a default");
    let ops = *sub.get_one::<u64>("ops").expect("ops has a default");
    match bench(seed, ops) {
        Ok(summary) => {
            print_bench(seed, ops, &summary, sub.get_flag("json"));
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

fn main() -> ExitCode {
    let matches = parser().get_matches();
    match matches.subcommand() {
        Some(("replay", sub)) => run_replay(sub),
        Some(("verify", sub)) => verify(sub),
        Some(("snapshot", sub)) => run_snapshot(sub),
        Some(("checkpoint", sub)) => run_checkpoint(sub),
        Some(("bench", sub)) => run_bench(sub),
        _ => ExitCode::from(2),
    }
}
