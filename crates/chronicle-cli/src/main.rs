//! chronicle-cli, the command line tool for chronicle books
//! (ARCHITECTURE.md section 2.3). One binary with four subcommands:
//! replay applies a write-ahead log to a book and prints the event
//! stream, verify reports the sequence and checksum health of a log
//! and snapshot pair, snapshot writes a snapshot from a log, and
//! bench runs a seeded synthetic load. This is the only crate in the
//! workspace that parses text or prints JSON. Output ordering is
//! fully determined by the input files and the seed.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

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

fn run_replay(_sub: &ArgMatches) -> ExitCode {
    panic!("M4 Stage A stub: replay lands in Stage B")
}

fn run_verify(_sub: &ArgMatches) -> ExitCode {
    panic!("M4 Stage A stub: verify lands in Stage B")
}

fn run_snapshot(_sub: &ArgMatches) -> ExitCode {
    panic!("M4 Stage A stub: snapshot lands in Stage B")
}

fn run_bench(_sub: &ArgMatches) -> ExitCode {
    panic!("M4 Stage A stub: bench lands in Stage B")
}

fn main() -> ExitCode {
    let matches = parser().get_matches();
    match matches.subcommand() {
        Some(("replay", sub)) => run_replay(sub),
        Some(("verify", sub)) => run_verify(sub),
        Some(("snapshot", sub)) => run_snapshot(sub),
        Some(("bench", sub)) => run_bench(sub),
        _ => ExitCode::from(2),
    }
}
