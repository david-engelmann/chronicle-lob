//! Operator command-line library and entry points for `chronicle-lob-cli`
//! (`docs/ARCHITECTURE.md` section 2.3).
//!
//! This crate powers the `chronicle-cli` executable and exposes its
//! deterministic argument parser, subcommand routines ([`replay`],
//! [`verify`], [`snapshot`], [`checkpoint`], [`bench`](fn@bench), and
//! [`write_completions`]), summary structures, [`SplitMix64`] workload
//! generator, and [`run_cli`] entry point so embedded callers, tests, and
//! `docs.rs` builds share one verified implementation.
//!
//! Output ordering is fully determined by the input files and `--seed`
//! (or `SIM_SEED`), and `#![forbid(unsafe_code)]` holds across the crate.
//!
//! # Examples
//!
//! ```
//! use std::process::ExitCode;
//! use chronicle_lob_cli::{bench, run_cli};
//!
//! let summary = bench(42, 1_000).expect("bench configuration is valid");
//! assert_eq!(summary.applied + summary.rejected, 1_000);
//!
//! let code = run_cli(["chronicle-cli", "bench", "--ops", "100", "--seed", "42", "--json"]);
//! assert_eq!(code, ExitCode::SUCCESS);
//! ```

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use chronicle_core::{
    Book, BookConfig, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, SnapshotState, StpPolicy,
};
use chronicle_wal::{
    ScannedFrame, WalError, decode_snapshot, load_snapshot, recover_from_reader, save_snapshot,
    scan_reader_with, snapshot_config,
};
use clap::{Arg, ArgMatches, Command};

/// Deterministic `SplitMix64` pseudo-random number generator (`docs/ARCHITECTURE.md` section 11).
///
/// Drives the synthetic [`bench`](fn@bench) command stream from an explicit 64-bit seed
/// without touching ambient randomness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Construct a generator seeded with `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance the state by one step and return the next 64-bit word.
    #[must_use]
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Summary counters produced by a deterministic [`bench`](fn@bench) run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchSummary {
    /// Number of generated commands accepted by the book.
    pub applied: u64,
    /// Number of generated commands rejected by preflight checks.
    pub rejected: u64,
    /// Total [`ExecutionEvent::Trade`] records emitted across accepted commands.
    pub trades: u64,
    /// Resting orders remaining on the book at the end of the run.
    pub live_orders: u32,
    /// Best bid price in quote units at the end of the run, if any.
    pub best_bid: Option<u64>,
    /// Best ask price in quote units at the end of the run, if any.
    pub best_ask: Option<u64>,
}

/// Verification report returned by [`verify`] for a healthy log and optional snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifySummary {
    /// `last_included_seq` from the verified base snapshot, when one was supplied.
    pub snapshot_last_included_seq: Option<u64>,
    /// Live order count in the verified base snapshot, when one was supplied.
    pub snapshot_live_orders: Option<usize>,
    /// Complete valid frames scanned in the write-ahead log.
    pub frames: u64,
    /// Trailing torn bytes discarded at a clean crash boundary.
    pub discarded_trailing_bytes: usize,
    /// Sequence number of the last complete frame, or `0` for an empty log.
    pub last_seq: u64,
}

/// Failure classification returned by [`verify`].
#[derive(Debug)]
pub enum VerifyError {
    /// Unreadable input path or invalid book configuration flags (CLI exit code `2`).
    Usage(String),
    /// Corrupt snapshot, corrupt WAL frame, or sequence gap (CLI exit code `1`).
    Corrupt {
        /// `(last_included_seq, live_orders)` if a base snapshot validated before WAL scanning failed.
        snapshot_ok: Option<(u64, usize)>,
        /// Underlying WAL or snapshot validation error.
        error: WalError,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(msg) => f.write_str(msg),
            Self::Corrupt { error, .. } => write!(f, "corrupt {error}"),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Usage(_) => None,
            Self::Corrupt { error, .. } => Some(error),
        }
    }
}

/// Summary returned by [`snapshot`] after writing an atomic snapshot file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotSummary {
    /// Highest command sequence covered by the written snapshot.
    pub last_included_seq: u64,
    /// Number of resting live orders persisted in the snapshot.
    pub live_orders: usize,
}

/// Summary returned by [`checkpoint`] after verifying the new snapshot and archiving the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointSummary {
    /// Highest command sequence covered by the checkpoint snapshot.
    pub last_included_seq: u64,
    /// Number of resting live orders persisted in the checkpoint snapshot.
    pub live_orders: usize,
    /// Destination path where the covered write-ahead log was archived.
    pub archived_log: PathBuf,
}

/// Target shell for deterministic `chronicle-cli` completion script generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shell {
    /// Bourne-again shell (`bash`).
    Bash,
    /// Z shell (`zsh`).
    Zsh,
    /// Friendly interactive shell (`fish`).
    Fish,
    /// `PowerShell` (`powershell` or `pwsh`).
    PowerShell,
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bash => f.write_str("bash"),
            Self::Zsh => f.write_str("zsh"),
            Self::Fish => f.write_str("fish"),
            Self::PowerShell => f.write_str("powershell"),
        }
    }
}

/// Error returned when parsing an unsupported shell name into [`Shell`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseShellError(String);

impl fmt::Display for ParseShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported shell '{}'; expected bash, zsh, fish, or powershell",
            self.0
        )
    }
}

impl std::error::Error for ParseShellError {}

impl FromStr for Shell {
    type Err = ParseShellError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bash" => Ok(Self::Bash),
            "zsh" => Ok(Self::Zsh),
            "fish" => Ok(Self::Fish),
            "powershell" | "pwsh" => Ok(Self::PowerShell),
            _ => Err(ParseShellError(s.to_string())),
        }
    }
}

const BASH_COMPLETIONS: &str = r#"_chronicle_cli() {
    local cur prev subcmd
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    case "${prev}" in
        --wal|--snapshot|--out)
            COMPREPLY=( $(compgen -f -- "${cur}") )
            return 0
            ;;
        --default-stp)
            COMPREPLY=( $(compgen -W "cancel-incoming cancel-resting cancel-both" -- "${cur}") )
            return 0
            ;;
        --seed|--ops|--max-orders|--grid-len|--tick-size|--grid-start-tick)
            return 0
            ;;
    esac

    subcmd=""
    local i=1
    while [[ ${i} -lt ${COMP_CWORD} ]]; do
        case "${COMP_WORDS[i]}" in
            replay|verify|snapshot|checkpoint|bench|completions)
                subcmd="${COMP_WORDS[i]}"
                break
                ;;
            --seed)
                i=$((i + 2))
                continue
                ;;
        esac
        i=$((i + 1))
    done

    if [[ -z "${subcmd}" ]]; then
        if [[ "${cur}" == -* ]]; then
            COMPREPLY=( $(compgen -W "--seed --help -h --version -V" -- "${cur}") )
        else
            COMPREPLY=( $(compgen -W "replay verify snapshot checkpoint bench completions" -- "${cur}") )
        fi
        return 0
    fi

    case "${subcmd}" in
        replay|verify)
            COMPREPLY=( $(compgen -W "--wal --snapshot --max-orders --grid-len --tick-size --grid-start-tick --default-stp --seed --help -h" -- "${cur}") )
            ;;
        snapshot|checkpoint)
            COMPREPLY=( $(compgen -W "--wal --snapshot --out --max-orders --grid-len --tick-size --grid-start-tick --default-stp --seed --help -h" -- "${cur}") )
            ;;
        bench)
            COMPREPLY=( $(compgen -W "--ops --json --seed --help -h" -- "${cur}") )
            ;;
        completions)
            if [[ "${cur}" == -* ]]; then
                COMPREPLY=( $(compgen -W "--seed --help -h" -- "${cur}") )
            else
                COMPREPLY=( $(compgen -W "bash zsh fish powershell pwsh" -- "${cur}") )
            fi
            ;;
    esac
    return 0
}

complete -F _chronicle_cli chronicle-cli
"#;

const ZSH_COMPLETIONS: &str = r#"#compdef chronicle-cli

_chronicle_cli() {
    local line state

    _arguments -C \
        '--seed[Seed for seeded work; honours SIM_SEED]:seed:' \
        '(-h --help)'{-h,--help}'[Print help]' \
        '(-V --version)'{-V,--version}'[Print version]' \
        '1:subcommand:((replay\:"Apply a write-ahead log and print the event stream" verify\:"Report the sequence and checksum health of a log and snapshot pair" snapshot\:"Write a snapshot from a write-ahead log and optional base snapshot" checkpoint\:"Write a verified snapshot and archive the log it covers" bench\:"Run a seeded synthetic load against a fresh book" completions\:"Write shell completion scripts to standard output"))' \
        '*::arg:->args'

    case "$state" in
        args)
            case "${line[1]}" in
                replay|verify)
                    _arguments \
                        '--wal[Write-ahead log file]:wal:_files' \
                        '--snapshot[Snapshot file supplying the base book and its configuration]:snapshot:_files' \
                        '--max-orders[Arena capacity of the book]:max_orders:' \
                        '--grid-len[Number of price ticks on the grid]:grid_len:' \
                        '--tick-size[Quote units per tick]:tick_size:' \
                        '--grid-start-tick[Tick value of the first grid level]:grid_start_tick:' \
                        '--default-stp[Default self-trade prevention policy]:policy:(cancel-incoming cancel-resting cancel-both)' \
                        '--seed[Seed for seeded work; honours SIM_SEED]:seed:' \
                        '(-h --help)'{-h,--help}'[Print help]'
                    ;;
                snapshot|checkpoint)
                    _arguments \
                        '--wal[Write-ahead log file]:wal:_files' \
                        '--snapshot[Snapshot file supplying the base book and its configuration]:snapshot:_files' \
                        '--out[Snapshot file to write]:out:_files' \
                        '--max-orders[Arena capacity of the book]:max_orders:' \
                        '--grid-len[Number of price ticks on the grid]:grid_len:' \
                        '--tick-size[Quote units per tick]:tick_size:' \
                        '--grid-start-tick[Tick value of the first grid level]:grid_start_tick:' \
                        '--default-stp[Default self-trade prevention policy]:policy:(cancel-incoming cancel-resting cancel-both)' \
                        '--seed[Seed for seeded work; honours SIM_SEED]:seed:' \
                        '(-h --help)'{-h,--help}'[Print help]'
                    ;;
                bench)
                    _arguments \
                        '--ops[Number of generated commands to apply]:ops:' \
                        '--json[Print the summary as JSON]' \
                        '--seed[Seed for seeded work; honours SIM_SEED]:seed:' \
                        '(-h --help)'{-h,--help}'[Print help]'
                    ;;
                completions)
                    _arguments \
                        '--seed[Seed for seeded work; honours SIM_SEED]:seed:' \
                        '(-h --help)'{-h,--help}'[Print help]' \
                        '1:shell:(bash zsh fish powershell pwsh)'
                    ;;
            esac
            ;;
    esac
}

_chronicle_cli "$@"
"#;

const FISH_COMPLETIONS: &str = r#"set -l __chronicle_subcommands replay verify snapshot checkpoint bench completions

complete -c chronicle-cli -f
complete -c chronicle-cli -l seed -r -d 'Seed for seeded work; honours SIM_SEED'
complete -c chronicle-cli -s h -l help -d 'Print help'
complete -c chronicle-cli -s V -l version -d 'Print version'

complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a replay -d 'Apply a write-ahead log and print the event stream'
complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a verify -d 'Report the sequence and checksum health of a log and snapshot pair'
complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a snapshot -d 'Write a snapshot from a write-ahead log and optional base snapshot'
complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a checkpoint -d 'Write a verified snapshot and archive the log it covers'
complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a bench -d 'Run a seeded synthetic load against a fresh book'
complete -c chronicle-cli -n "not __fish_seen_subcommand_from $__chronicle_subcommands" -a completions -d 'Write shell completion scripts to standard output'

for sub in replay verify snapshot checkpoint
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l wal -r -F -d 'Write-ahead log file'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l snapshot -r -F -d 'Snapshot file supplying the base book and its configuration'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l max-orders -r -d 'Arena capacity of the book'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l grid-len -r -d 'Number of price ticks on the grid'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l tick-size -r -d 'Quote units per tick'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l grid-start-tick -r -d 'Tick value of the first grid level'
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l default-stp -r -a 'cancel-incoming cancel-resting cancel-both' -d 'Default self-trade prevention policy'
end

for sub in snapshot checkpoint
    complete -c chronicle-cli -n "__fish_seen_subcommand_from $sub" -l out -r -F -d 'Snapshot file to write'
end

complete -c chronicle-cli -n '__fish_seen_subcommand_from bench' -l ops -r -d 'Number of generated commands to apply'
complete -c chronicle-cli -n '__fish_seen_subcommand_from bench' -l json -d 'Print the summary as JSON'

complete -c chronicle-cli -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish powershell pwsh' -d 'Target shell'
"#;

const POWERSHELL_COMPLETIONS: &str = r#"Register-ArgumentCompleter -Native -CommandName 'chronicle-cli' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $tokens = $commandAst.CommandElements | ForEach-Object { $_.ToString() }
    $prev = if ($tokens.Count -ge 2 -and $wordToComplete -ne '') {
        $tokens[$tokens.Count - 2]
    } elseif ($tokens.Count -ge 1 -and $wordToComplete -eq '') {
        $tokens[$tokens.Count - 1]
    } else {
        ''
    }

    if ($prev -eq '--default-stp') {
        @('cancel-incoming', 'cancel-resting', 'cancel-both') |
            Where-Object { $_ -like "$wordToComplete*" } |
            ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
        return
    }

    $subcommands = @('replay', 'verify', 'snapshot', 'checkpoint', 'bench', 'completions')
    $subcmd = $null
    for ($i = 1; $i -lt $tokens.Count; $i++) {
        if ($subcommands -contains $tokens[$i] -and $tokens[$i] -ne $wordToComplete) {
            $subcmd = $tokens[$i]
            break
        }
    }

    $candidates = if ($null -eq $subcmd) {
        if ($wordToComplete -like '-*') {
            @('--seed', '--help', '-h', '--version', '-V')
        } else {
            $subcommands
        }
    } else {
        switch ($subcmd) {
            'replay' { @('--wal', '--snapshot', '--max-orders', '--grid-len', '--tick-size', '--grid-start-tick', '--default-stp', '--seed', '--help', '-h') }
            'verify' { @('--wal', '--snapshot', '--max-orders', '--grid-len', '--tick-size', '--grid-start-tick', '--default-stp', '--seed', '--help', '-h') }
            'snapshot' { @('--wal', '--snapshot', '--out', '--max-orders', '--grid-len', '--tick-size', '--grid-start-tick', '--default-stp', '--seed', '--help', '-h') }
            'checkpoint' { @('--wal', '--snapshot', '--out', '--max-orders', '--grid-len', '--tick-size', '--grid-start-tick', '--default-stp', '--seed', '--help', '-h') }
            'bench' { @('--ops', '--json', '--seed', '--help', '-h') }
            'completions' {
                if ($wordToComplete -like '-*') {
                    @('--seed', '--help', '-h')
                } else {
                    @('bash', 'zsh', 'fish', 'powershell', 'pwsh')
                }
            }
        }
    }

    $candidates |
        Where-Object { $_ -like "$wordToComplete*" } |
        ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterName', $_) }
}
"#;

/// Return the static shell completion script for `shell`.
#[must_use]
pub const fn completions_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => BASH_COMPLETIONS,
        Shell::Zsh => ZSH_COMPLETIONS,
        Shell::Fish => FISH_COMPLETIONS,
        Shell::PowerShell => POWERSHELL_COMPLETIONS,
    }
}

/// Write the deterministic shell completion script for `shell` to `out`.
///
/// # Errors
///
/// Propagates any [`io::Error`] returned while writing or flushing `out`.
pub fn write_completions<W: Write>(shell: Shell, out: &mut W) -> io::Result<()> {
    out.write_all(completions_script(shell).as_bytes())?;
    out.flush()
}

/// Build the `clap` argument parser for `chronicle-cli`.
///
/// Every subcommand accepts `--seed`, which honours the `SIM_SEED`
/// environment variable (`docs/ARCHITECTURE.md` section 2.3).
#[must_use]
pub fn parser() -> Command {
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
        .subcommand(
            Command::new("completions")
                .about("Write shell completion scripts to standard output")
                .arg(
                    Arg::new("shell")
                        .value_name("SHELL")
                        .required(true)
                        .ignore_case(true)
                        .value_parser(["bash", "zsh", "fish", "powershell", "pwsh"])
                        .help("Target shell (bash, zsh, fish, powershell)"),
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

/// Extract [`BookConfig`] from validated subcommand matches.
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

/// Format one [`ExecutionEvent`] in the `docs/ARCHITECTURE.md` section 2.3
/// line wire format directly into `out` without allocating an intermediate `String`.
///
/// # Errors
///
/// Propagates any [`io::Error`] returned while writing to `out`.
pub fn write_event(out: &mut impl Write, event: &ExecutionEvent) -> io::Result<()> {
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

/// Stream `wal_path` through a book initialised from `base_snapshot` (or a
/// fresh `config` book) and write `seed={seed}` followed by formatted execution
/// events to `out`.
///
/// Broken pipes on `out` terminate the stream cleanly without reporting an
/// error so downstream filters such as `head` exit with status `0`.
///
/// # Errors
///
/// Returns an error description if the book configuration or snapshot is
/// invalid, `wal_path` cannot be opened, a WAL frame is corrupt or out of
/// sequence, a command is rejected by the book, or writing to `out` fails with
/// an error other than [`io::ErrorKind::BrokenPipe`].
pub fn replay(
    wal_path: &Path,
    config: BookConfig,
    base_snapshot: Option<SnapshotState>,
    seed: u64,
    out: &mut impl Write,
) -> Result<(), String> {
    let (mut book, base_seq) = match base_snapshot {
        Some(state) => {
            let base_seq = state
                .next_seq
                .checked_sub(1)
                .ok_or_else(|| "snapshot next_seq must be at least 1".to_string())?;
            let book = Book::from_snapshot_state(state).map_err(|e| e.to_string())?;
            (book, base_seq)
        }
        None => (Book::new(config).map_err(|e| e.to_string())?, 0),
    };
    let file = fs::File::open(wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", wal_path.display()))?;
    // INV-DET-2: the seed banner prints at startup, before any
    // event line, buffered alongside the event stream.
    if let Err(err) = writeln!(out, "seed={seed}") {
        return if err.kind() == io::ErrorKind::BrokenPipe {
            Ok(())
        } else {
            Err(format!("cannot write replay output: {err}"))
        };
    }
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(2 * config.max_orders as usize + 2);
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
            write_event(out, event).map_err(|source| WalError::Io {
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
    let seed = *sub.get_one::<u64>("seed").expect("seed has a default");
    let resolved = match inputs(sub) {
        Ok(inputs) => inputs,
        Err(message) => return fail(&message),
    };
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    match replay(
        &resolved.wal_path,
        resolved.config,
        resolved.base_snapshot,
        seed,
        &mut out,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

/// Verify sequence continuity and Castagnoli CRC32C checksums of `wal_path`
/// and an optional `snapshot_path` in bounded memory.
///
/// When `snapshot_path` is `None`, `config` is validated before scanning
/// `wal_path`.
///
/// # Errors
///
/// Returns [`VerifyError::Usage`] if an input file cannot be opened or
/// `config` is missing or invalid, and [`VerifyError::Corrupt`] if either the
/// snapshot or the write-ahead log fails structural, checksum, or sequence
/// validation.
pub fn verify(
    wal_path: &Path,
    snapshot_path: Option<&Path>,
    config: Option<BookConfig>,
) -> Result<VerifySummary, VerifyError> {
    let (base_seq, snapshot_ok) = if let Some(path) = snapshot_path {
        let bytes = fs::read(path).map_err(|e| {
            VerifyError::Usage(format!("cannot read snapshot {}: {e}", path.display()))
        })?;
        let snap_cfg = snapshot_config(&bytes).map_err(|error| VerifyError::Corrupt {
            snapshot_ok: None,
            error,
        })?;
        let state = decode_snapshot(&bytes, &snap_cfg).map_err(|error| VerifyError::Corrupt {
            snapshot_ok: None,
            error,
        })?;
        let covered_seq = state.next_seq.saturating_sub(1);
        (covered_seq, Some((covered_seq, state.live_orders.len())))
    } else {
        let cfg = config.ok_or_else(|| {
            VerifyError::Usage("either snapshot_path or config must be supplied".to_string())
        })?;
        cfg.validate()
            .map_err(|e| VerifyError::Usage(e.to_string()))?;
        (0u64, None)
    };
    let file = fs::File::open(wal_path)
        .map_err(|e| VerifyError::Usage(format!("cannot read log {}: {e}", wal_path.display())))?;
    let mut seq_check = SequenceChecker::new(base_seq);
    match scan_reader_with(file, |frame| seq_check.check(&frame)) {
        Ok(report) => Ok(VerifySummary {
            snapshot_last_included_seq: snapshot_ok.map(|(seq, _)| seq),
            snapshot_live_orders: snapshot_ok.map(|(_, live)| live),
            frames: report.frame_count,
            discarded_trailing_bytes: report.discarded_trailing_bytes,
            last_seq: report.last_seq.unwrap_or(0),
        }),
        Err(error) => Err(VerifyError::Corrupt { snapshot_ok, error }),
    }
}

fn run_verify(sub: &ArgMatches) -> ExitCode {
    let wal_path = sub
        .get_one::<PathBuf>("wal")
        .expect("the parser requires --wal");
    let snapshot_path = sub.get_one::<PathBuf>("snapshot").map(PathBuf::as_path);
    let config = if snapshot_path.is_none() {
        Some(config_from_flags(sub))
    } else {
        None
    };
    match verify(wal_path, snapshot_path, config) {
        Ok(summary) => {
            if let (Some(seq), Some(live)) = (
                summary.snapshot_last_included_seq,
                summary.snapshot_live_orders,
            ) {
                println!("snapshot: ok last_included_seq={seq} live_orders={live}");
            }
            println!(
                "wal: ok frames={} discarded={} last_seq={}",
                summary.frames, summary.discarded_trailing_bytes, summary.last_seq
            );
            println!("verdict: ok");
            ExitCode::SUCCESS
        }
        Err(VerifyError::Usage(message)) => fail(&message),
        Err(VerifyError::Corrupt { snapshot_ok, error }) => {
            if let Some((seq, live)) = snapshot_ok {
                println!("snapshot: ok last_included_seq={seq} live_orders={live}");
            }
            corrupt(&error)
        }
    }
}

/// Recover a book from `wal_path` (and optional `base_snapshot`) and write an
/// atomic snapshot to `out_path`.
///
/// # Errors
///
/// Returns an error description if `wal_path` cannot be opened, recovery
/// fails, or saving `out_path` fails.
pub fn snapshot(
    wal_path: &Path,
    out_path: &Path,
    config: BookConfig,
    base_snapshot: Option<SnapshotState>,
) -> Result<SnapshotSummary, String> {
    let file = fs::File::open(wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", wal_path.display()))?;
    let recovery = recover_from_reader(config, base_snapshot, file).map_err(|e| e.to_string())?;
    let state = recovery.book.capture_snapshot_state();
    save_snapshot(out_path, &state).map_err(|e| e.to_string())?;
    Ok(SnapshotSummary {
        last_included_seq: state.next_seq.saturating_sub(1),
        live_orders: state.live_orders.len(),
    })
}

fn run_snapshot(sub: &ArgMatches) -> ExitCode {
    let resolved = match inputs(sub) {
        Ok(inputs) => inputs,
        Err(message) => return fail(&message),
    };
    let out_path = sub
        .get_one::<PathBuf>("out")
        .expect("the parser requires --out");
    match snapshot(
        &resolved.wal_path,
        out_path,
        resolved.config,
        resolved.base_snapshot,
    ) {
        Ok(summary) => {
            println!(
                "snapshot: written last_included_seq={} live_orders={}",
                summary.last_included_seq, summary.live_orders
            );
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

/// Compute the archive path `<wal>.archive-<last_included_seq>` for a
/// checkpointed write-ahead log so successive checkpoints archive under
/// distinct names.
#[must_use]
pub fn archive_path_for(wal_path: &Path, last_included_seq: u64) -> PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(format!(".archive-{last_included_seq}"));
    PathBuf::from(name)
}

/// Write a verified checkpoint snapshot to `out_path` and archive `wal_path`.
///
/// Recovers a book from `wal_path` (and optional `base_snapshot`), writes an
/// atomic snapshot to `out_path`, verifies the snapshot by decoding it back
/// from disk, and renames `wal_path` to its covered archive path. Because
/// `base_snapshot` is already held in memory, `out_path` may safely point to
/// the same file that supplied `base_snapshot`.
///
/// # Errors
///
/// Returns an error description if recovery, snapshot persistence, reload
/// verification, or log archiving fails. The live log is left untouched until
/// reload verification succeeds (`docs/ARCHITECTURE.md` section 10.6).
pub fn checkpoint(
    wal_path: &Path,
    out_path: &Path,
    config: BookConfig,
    base_snapshot: Option<SnapshotState>,
) -> Result<CheckpointSummary, String> {
    let file = fs::File::open(wal_path)
        .map_err(|e| format!("cannot read log {}: {e}", wal_path.display()))?;
    let recovery = recover_from_reader(config, base_snapshot, file).map_err(|e| e.to_string())?;
    let state = recovery.book.capture_snapshot_state();
    let last_included_seq = state.next_seq.saturating_sub(1);
    save_snapshot(out_path, &state).map_err(|e| e.to_string())?;
    // Verify before archiving (ARCHITECTURE.md section 10.6): the
    // snapshot on disk must load and name the sequence the log
    // reached. Until this passes, the log stays where it is.
    let loaded = load_snapshot(out_path, &config).map_err(|e| e.to_string())?;
    if loaded.next_seq != state.next_seq {
        return Err(format!(
            "checkpoint verification failed: the snapshot names next_seq {} but the log reached {}",
            loaded.next_seq, state.next_seq
        ));
    }
    let archive_path = archive_path_for(wal_path, last_included_seq);
    if archive_path.exists() {
        fs::remove_file(&archive_path).map_err(|e| {
            format!(
                "cannot replace archived log {}: {e}",
                archive_path.display()
            )
        })?;
    }
    fs::rename(wal_path, &archive_path)
        .map_err(|e| format!("cannot archive log {}: {e}", wal_path.display()))?;
    Ok(CheckpointSummary {
        last_included_seq,
        live_orders: state.live_orders.len(),
        archived_log: archive_path,
    })
}

fn run_checkpoint(sub: &ArgMatches) -> ExitCode {
    // `inputs` reads and decodes the optional `--snapshot` completely
    // in memory before opening the WAL or writing `--out`, so `--snapshot
    // foo.snap --out foo.snap` updates in place safely through
    // `save_snapshot`'s atomic `.tmp` write, fsync, and rename.
    let resolved = match inputs(sub) {
        Ok(inputs) => inputs,
        Err(message) => return fail(&message),
    };
    let out_path = sub
        .get_one::<PathBuf>("out")
        .expect("the parser requires --out");
    match checkpoint(
        &resolved.wal_path,
        out_path,
        resolved.config,
        resolved.base_snapshot,
    ) {
        Ok(summary) => {
            println!(
                "checkpoint: written last_included_seq={} live_orders={} archived_log={}",
                summary.last_included_seq,
                summary.live_orders,
                summary.archived_log.display()
            );
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

/// Fixed [`BookConfig`] used by [`bench`](fn@bench): large enough (`65,536` slots, `6,400`
/// ticks) that the 5,000-order synthetic ID space keeps thousands of orders live.
#[must_use]
pub const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 65_536,
        grid_len: 6400,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// Execute `ops` deterministic synthetic commands seeded with `seed` against a
/// fresh [`bench_config`] book.
///
/// Generates a reproducible mix of `Limit`, `Ioc`, `Fok`, and `PostOnly` adds,
/// cancels, amends, participant mass cancels, and zero-quantity rejections
/// over a 5,000-order ID space using [`SplitMix64`].
///
/// # Errors
///
/// Returns an error description only if constructing the benchmark book fails.
pub fn bench(seed: u64, ops: u64) -> Result<BenchSummary, String> {
    let mut book = Book::new(bench_config()).map_err(|e| e.to_string())?;
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(32_768);
    let mut rng = SplitMix64::new(seed);
    let mut summary = BenchSummary {
        applied: 0,
        rejected: 0,
        trades: 0,
        live_orders: 0,
        best_bid: None,
        best_ask: None,
    };
    for _ in 0..ops {
        let r = rng.next_u64();
        let slot = r % 100;
        let participant = ParticipantId(u32::try_from((r >> 24) % 8).unwrap_or(0));
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

fn run_completions(sub: &ArgMatches) -> ExitCode {
    let raw = sub
        .get_one::<String>("shell")
        .expect("the parser requires SHELL");
    let Ok(shell) = raw.parse::<Shell>() else {
        return fail(&format!(
            "unsupported shell '{raw}'; expected bash, zsh, fish, or powershell"
        ));
    };
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    match write_completions(shell, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => fail(&format!("cannot write completions: {err}")),
    }
}

/// Parse `args` (including `argv[0]`) and execute the selected `chronicle-cli`
/// subcommand, returning its process [`ExitCode`] without calling
/// `std::process::exit`.
pub fn run_cli<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = match parser().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(err) => {
            let _ = err.print();
            let code = u8::try_from(err.exit_code()).unwrap_or(2);
            return ExitCode::from(code);
        }
    };
    match matches.subcommand() {
        Some(("replay", sub)) => run_replay(sub),
        Some(("verify", sub)) => run_verify(sub),
        Some(("snapshot", sub)) => run_snapshot(sub),
        Some(("checkpoint", sub)) => run_checkpoint(sub),
        Some(("bench", sub)) => run_bench(sub),
        Some(("completions", sub)) => run_completions(sub),
        _ => ExitCode::from(2),
    }
}

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
