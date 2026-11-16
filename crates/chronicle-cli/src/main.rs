//! chronicle-cli, the command line tool for chronicle books
//! (ARCHITECTURE.md section 2.3). One binary with four subcommands:
//! replay applies a write-ahead log to a book and prints the event
//! stream, verify reports the sequence and checksum health of a log
//! and snapshot pair, snapshot writes a snapshot from a log, and
//! bench runs a seeded synthetic load. This is the only crate in the
//! workspace that parses text or prints JSON. Output ordering is
//! fully determined by the input files and the seed.

#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use chronicle_core::{
    Book, BookConfig, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, StpPolicy,
};
use chronicle_wal::{
    ScannedFrame, WalError, load_snapshot, recover, save_snapshot, scan_log, snapshot_config,
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
        .about("Replay, verify, snapshot, and benchmark chronicle books")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(seed)
        .subcommand(
            Command::new("replay")
                .about("Apply a write-ahead log and print the event stream")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .args(config_args(true)),
        )
        .subcommand(
            Command::new("verify")
                .about("Report the sequence and checksum health of a log and snapshot pair")
                .arg(wal_arg())
                .arg(snapshot_arg())
                .args(config_args(true)),
        )
        .subcommand(
            Command::new("snapshot")
                .about("Write a snapshot from a write-ahead log")
                .arg(wal_arg())
                .arg(
                    Arg::new("out")
                        .long("out")
                        .value_name("PATH")
                        .value_parser(clap::value_parser!(PathBuf))
                        .required(true)
                        .help("Snapshot file to write"),
                )
                .args(config_args(false)),
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

/// The book configuration flags. When the subcommand also accepts a
/// snapshot, the snapshot supplies the configuration instead: the
/// flags then conflict with `--snapshot` and are required without
/// one. When it does not, the flags are simply required.
fn config_args(with_snapshot: bool) -> Vec<Arg> {
    let mut max_orders = Arg::new("max-orders")
        .long("max-orders")
        .value_name("U32")
        .value_parser(clap::value_parser!(u32))
        .help("Arena capacity of the book");
    let mut grid_len = Arg::new("grid-len")
        .long("grid-len")
        .value_name("U32")
        .value_parser(clap::value_parser!(u32))
        .help("Number of price ticks on the grid");
    let mut tick_size = Arg::new("tick-size")
        .long("tick-size")
        .value_name("U64")
        .value_parser(clap::value_parser!(u64))
        .help("Quote units per tick");
    let mut grid_start_tick = Arg::new("grid-start-tick")
        .long("grid-start-tick")
        .value_name("U64")
        .value_parser(clap::value_parser!(u64))
        .help("Tick value of the first grid level");
    let mut default_stp = Arg::new("default-stp")
        .long("default-stp")
        .value_name("POLICY")
        .value_parser(["cancel-incoming", "cancel-resting", "cancel-both"])
        .help("Default self-trade prevention policy");
    if with_snapshot {
        max_orders = max_orders
            .required_unless_present("snapshot")
            .conflicts_with("snapshot");
        grid_len = grid_len
            .required_unless_present("snapshot")
            .conflicts_with("snapshot");
        tick_size = tick_size
            .required_unless_present("snapshot")
            .conflicts_with("snapshot");
        grid_start_tick = grid_start_tick
            .required_unless_present("snapshot")
            .conflicts_with("snapshot");
        default_stp = default_stp.conflicts_with("snapshot");
    } else {
        max_orders = max_orders.required(true);
        grid_len = grid_len.required(true);
        tick_size = tick_size.required(true);
        grid_start_tick = grid_start_tick.required(true);
    }
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

/// The sequence continuity check shared by replay and verify:
/// frames at or below `base_seq` are covered by the base snapshot,
/// and the rest must step by exactly one from `base_seq + 1`.
fn sequence_gap(frames: &[ScannedFrame], base_seq: u64) -> Option<WalError> {
    let mut expected = base_seq + 1;
    for frame in frames {
        if frame.seq <= base_seq {
            continue;
        }
        if frame.seq != expected {
            return Some(WalError::WalSequenceGap {
                expected,
                found: frame.seq,
                offset: frame.offset,
            });
        }
        expected += 1;
    }
    None
}

struct Inputs {
    wal_path: PathBuf,
    config: BookConfig,
    snapshot_path: Option<PathBuf>,
}

/// Resolve the inputs of replay: paths, and the configuration,
/// which a supplied snapshot carries in its own header.
fn inputs(sub: &ArgMatches) -> Result<Inputs, String> {
    let wal_path = sub
        .get_one::<PathBuf>("wal")
        .expect("the parser requires --wal")
        .clone();
    let snapshot_path = sub.get_one::<PathBuf>("snapshot").cloned();
    let config = match &snapshot_path {
        Some(path) => {
            let bytes = fs::read(path)
                .map_err(|e| format!("cannot read snapshot {}: {e}", path.display()))?;
            snapshot_config(&bytes).map_err(|e| e.to_string())?
        }
        None => config_from_flags(sub),
    };
    Ok(Inputs {
        wal_path,
        config,
        snapshot_path,
    })
}

fn replay(sub: &ArgMatches) -> Result<(), String> {
    // INV-DET-2: the seed banner prints at startup, before any
    // event line, exactly as the bench text output opens.
    let seed = *sub.get_one::<u64>("seed").expect("seed has a default");
    println!("seed={seed}");
    let inputs = inputs(sub)?;
    let (mut book, base_seq) = match &inputs.snapshot_path {
        Some(path) => {
            let state = load_snapshot(path, &inputs.config).map_err(|e| e.to_string())?;
            let base_seq = state.next_seq - 1;
            let book = Book::from_snapshot_state(state).map_err(|e| e.to_string())?;
            (book, base_seq)
        }
        None => (Book::new(inputs.config).map_err(|e| e.to_string())?, 0),
    };
    let bytes = fs::read(&inputs.wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", inputs.wal_path.display()))?;
    let outcome = scan_log(&bytes).map_err(|e| e.to_string())?;
    if let Some(gap) = sequence_gap(&outcome.frames, base_seq) {
        return Err(gap.to_string());
    }
    let mut storage: Vec<ExecutionEvent> =
        Vec::with_capacity(2 * inputs.config.max_orders as usize + 2);
    for frame in &outcome.frames {
        if frame.seq <= base_seq {
            continue;
        }
        let mut events = EventBuffer::new(&mut storage);
        let written = frame.command.apply(&mut book, &mut events).map_err(|e| {
            format!(
                "frame at offset {} (seq {}) rejected: {e}",
                frame.offset, frame.seq
            )
        })?;
        for event in &storage[..written] {
            println!("{}", format_event(event));
        }
        storage.clear();
    }
    Ok(())
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
        match load_snapshot(path, &config) {
            Ok(state) => {
                base_seq = state.next_seq - 1;
                println!(
                    "snapshot: ok last_included_seq={} live_orders={}",
                    state.next_seq - 1,
                    state.live_orders.len()
                );
            }
            Err(e) => return corrupt(&e),
        }
    } else if let Err(e) = Book::new(config_from_flags(sub)) {
        return fail(&e.to_string());
    }
    let bytes = match fs::read(&wal_path) {
        Ok(bytes) => bytes,
        Err(e) => return fail(&format!("cannot read log {}: {e}", wal_path.display())),
    };
    match scan_log(&bytes) {
        Ok(outcome) => {
            if let Some(gap) = sequence_gap(&outcome.frames, base_seq) {
                return corrupt(&gap);
            }
            let last_seq = outcome.frames.last().map_or(0, |frame| frame.seq);
            println!(
                "wal: ok frames={} discarded={} last_seq={}",
                outcome.frames.len(),
                outcome.discarded_trailing_bytes,
                last_seq
            );
            println!("verdict: ok");
            ExitCode::SUCCESS
        }
        Err(e) => corrupt(&e),
    }
}

fn snapshot_command(sub: &ArgMatches) -> Result<(), String> {
    let config = config_from_flags(sub);
    let wal_path = sub
        .get_one::<PathBuf>("wal")
        .expect("the parser requires --wal")
        .clone();
    let out_path = sub
        .get_one::<PathBuf>("out")
        .expect("the parser requires --out")
        .clone();
    let bytes =
        fs::read(&wal_path).map_err(|e| format!("cannot read log {}: {e}", wal_path.display()))?;
    let recovery = recover(config, None, &bytes).map_err(|e| e.to_string())?;
    let state = recovery.book.capture_snapshot_state();
    save_snapshot(&out_path, &state).map_err(|e| e.to_string())?;
    println!(
        "snapshot: written last_included_seq={} live_orders={}",
        state.next_seq - 1,
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
        Some(("bench", sub)) => run_bench(sub),
        _ => ExitCode::from(2),
    }
}
