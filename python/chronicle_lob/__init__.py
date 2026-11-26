"""Typed Python SDK, CWAL binary encoder, and CLI wrapper for ``chronicle-lob``."""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import sysconfig
from typing import Sequence

__all__ = [
    "__version__",
    "BenchReport",
    "BookSpec",
    "CheckpointReport",
    "ChronicleError",
    "ReplayEvent",
    "ReplayReport",
    "SnapshotReport",
    "VerifyReport",
    "WalBuilder",
    "bench",
    "checkpoint",
    "crc32c",
    "find_binary",
    "replay",
    "snapshot",
    "verify",
    "version",
]

__version__ = "0.3.0"

PathInput = str | os.PathLike[str]

_WAL_DOMAIN = b"chronicle-wal/v1\x00"
_MAGIC = b"CWAL"
_POLYNOMIAL = 0x82F63B78


def _build_crc32c_table() -> tuple[int, ...]:
    table: list[int] = []
    for i in range(256):
        crc = i
        for _ in range(8):
            crc = ((crc >> 1) ^ _POLYNOMIAL) if (crc & 1) else (crc >> 1)
        table.append(crc)
    return tuple(table)


_CRC32C_TABLE = _build_crc32c_table()

_SIDE_BYTES = {"bid": 0, "ask": 1}
_ORDER_TYPE_BYTES = {"limit": 0, "ioc": 1, "fok": 2, "post-only": 3, "post_only": 3}
_STP_BYTES = {
    None: 255,
    "cancel-incoming": 0,
    "cancel_incoming": 0,
    "cancel-resting": 1,
    "cancel_resting": 1,
    "cancel-both": 2,
    "cancel_both": 2,
}


def crc32c(data: bytes, initial: int = 0xFFFFFFFF) -> int:
    """Compute Castagnoli CRC32C matching ``chronicle-wal``."""
    crc = initial
    table = _CRC32C_TABLE
    for byte in data:
        crc = table[(crc ^ byte) & 0xFF] ^ (crc >> 8)
    return (~crc) & 0xFFFFFFFF


def _frame_crc(header_prefix: bytes, payload: bytes) -> int:
    crc = 0xFFFFFFFF
    table = _CRC32C_TABLE
    for chunk in (_WAL_DOMAIN, header_prefix, payload):
        for byte in chunk:
            crc = table[(crc ^ byte) & 0xFF] ^ (crc >> 8)
    return (~crc) & 0xFFFFFFFF


class ChronicleError(RuntimeError):
    """Raised when ``chronicle-cli`` reports corruption or invalid arguments."""

    def __init__(self, message: str, *, exit_code: int = 2) -> None:
        super().__init__(message)
        self.exit_code = exit_code


@dataclass(frozen=True, slots=True)
class BookSpec:
    """Book grid and arena configuration mirroring ``chronicle_core::BookConfig``."""

    max_orders: int
    grid_len: int
    tick_size: int
    grid_start_tick: int
    default_stp: str = "cancel-incoming"

    def to_cli_args(self) -> list[str]:
        """Render the CLI flags expected by ``chronicle-cli``."""
        return [
            "--max-orders",
            str(self.max_orders),
            "--grid-len",
            str(self.grid_len),
            "--tick-size",
            str(self.tick_size),
            "--grid-start-tick",
            str(self.grid_start_tick),
            "--default-stp",
            self.default_stp,
        ]


@dataclass(frozen=True, slots=True)
class BenchReport:
    """Deterministic synthetic benchmark summary from ``chronicle-cli bench``."""

    seed: int
    ops: int
    applied: int
    rejected: int
    trades: int
    live_orders: int
    best_bid: int | None
    best_ask: int | None


@dataclass(frozen=True, slots=True)
class VerifyReport:
    """Verification outcome from ``chronicle-cli verify``."""

    ok: bool
    frames: int
    discarded_trailing_bytes: int
    last_seq: int
    snapshot_last_included_seq: int | None
    snapshot_live_orders: int | None
    detail: str
    exit_code: int


@dataclass(frozen=True, slots=True)
class ReplayEvent:
    """One deterministic execution event emitted by ``chronicle-cli replay``."""

    seq: int
    kind: str
    fields: dict[str, int | str]


@dataclass(frozen=True, slots=True)
class ReplayReport:
    """Full event stream produced by ``chronicle-cli replay``."""

    seed: int
    events: tuple[ReplayEvent, ...]


@dataclass(frozen=True, slots=True)
class SnapshotReport:
    """Snapshot write summary from ``chronicle-cli snapshot``."""

    last_included_seq: int
    live_orders: int
    out_path: Path


@dataclass(frozen=True, slots=True)
class CheckpointReport:
    """Checkpoint summary from ``chronicle-cli checkpoint``."""

    last_included_seq: int
    live_orders: int
    out_path: Path
    archived_log: Path


class WalBuilder:
    """Pure-Python ``CWAL`` v1 binary log encoder compatible with ``chronicle-wal``."""

    def __init__(self, start_seq: int = 1) -> None:
        if start_seq < 1:
            raise ValueError("start_seq must be >= 1")
        self._next_seq = start_seq
        self._buffer = bytearray()

    @property
    def next_seq(self) -> int:
        """Next sequence number that will be assigned to an appended frame."""
        return self._next_seq

    def _append_frame(self, kind: int, payload: bytes, seq: int | None = None) -> int:
        assigned = self._next_seq if seq is None else seq
        if assigned < 1:
            raise ValueError("frame sequence must be >= 1")
        header_prefix = struct.pack(
            "<4sHHQIB7s",
            _MAGIC,
            1,
            32,
            assigned,
            len(payload),
            kind,
            b"\x00" * 7,
        )
        checksum = _frame_crc(header_prefix, payload)
        self._buffer.extend(header_prefix)
        self._buffer.extend(struct.pack("<I", checksum))
        self._buffer.extend(payload)
        self._next_seq = max(self._next_seq, assigned + 1)
        return assigned

    def add(
        self,
        *,
        order_id: int,
        participant: int,
        side: str,
        price: int,
        qty: int,
        order_type: str = "limit",
        stp: str | None = None,
        seq: int | None = None,
    ) -> int:
        """Append an ``Add`` command frame and return its sequence number."""
        side_byte = _SIDE_BYTES.get(side.lower())
        if side_byte is None:
            raise ValueError(f"side must be 'bid' or 'ask', got {side!r}")
        type_byte = _ORDER_TYPE_BYTES.get(order_type.lower())
        if type_byte is None:
            raise ValueError(f"unsupported order_type {order_type!r}")
        stp_key = stp.lower() if isinstance(stp, str) else None
        if stp_key not in _STP_BYTES:
            raise ValueError(f"unsupported stp policy {stp!r}")
        payload = struct.pack(
            "<QIBBBxQQ8x",
            order_id,
            participant,
            side_byte,
            type_byte,
            _STP_BYTES[stp_key],
            price,
            qty,
        )
        return self._append_frame(1, payload, seq=seq)

    def cancel(self, *, order_id: int, seq: int | None = None) -> int:
        """Append a ``Cancel`` command frame and return its sequence number."""
        payload = struct.pack("<Q8x", order_id)
        return self._append_frame(2, payload, seq=seq)

    def cancel_by_participant(self, *, participant: int, seq: int | None = None) -> int:
        """Append a ``CancelByParticipant`` command frame."""
        payload = struct.pack("<I12x", participant)
        return self._append_frame(3, payload, seq=seq)

    def amend(
        self,
        *,
        order_id: int,
        new_price: int,
        new_qty: int,
        seq: int | None = None,
    ) -> int:
        """Append an ``Amend`` command frame and return its sequence number."""
        payload = struct.pack("<QQQ8x", order_id, new_price, new_qty)
        return self._append_frame(4, payload, seq=seq)

    def to_bytes(self) -> bytes:
        """Return the encoded binary log bytes."""
        return bytes(self._buffer)

    def write(self, path: PathInput) -> Path:
        """Write the encoded WAL bytes to ``path`` and return it."""
        target = Path(path)
        target.write_bytes(self._buffer)
        return target


def find_binary(binary: PathInput | None = None) -> Path:
    """Locate the bundled or installed ``chronicle-cli`` executable."""
    if binary is not None:
        candidate = Path(binary)
        if candidate.is_file():
            return candidate
        raise FileNotFoundError(f"chronicle-cli binary not found at {candidate}")

    env_override = os.environ.get("CHRONICLE_CLI_BIN", "").strip()
    if env_override:
        candidate = Path(env_override)
        if candidate.is_file():
            return candidate
        raise FileNotFoundError(
            f"CHRONICLE_CLI_BIN points to missing file {candidate}"
        )

    exe_names = (
        ("chronicle-cli.exe", "chronicle-cli")
        if os.name == "nt"
        else ("chronicle-cli",)
    )
    search_dirs: list[Path] = []
    scripts_dir = sysconfig.get_path("scripts")
    if scripts_dir:
        search_dirs.append(Path(scripts_dir))
    py_dir = Path(sys.executable).resolve().parent
    search_dirs.extend([py_dir, py_dir / "Scripts", py_dir / "bin"])

    repo_root = Path(__file__).resolve().parents[2]
    search_dirs.extend(
        [
            repo_root / "target" / "release",
            repo_root / "target" / "debug",
        ]
    )

    for directory in search_dirs:
        for name in exe_names:
            candidate = directory / name
            if candidate.is_file():
                return candidate

    found = shutil.which("chronicle-cli")
    if found:
        return Path(found)

    raise FileNotFoundError(
        "could not locate chronicle-cli; install chronicle-lob or set CHRONICLE_CLI_BIN"
    )


def _run_cli(
    args: Sequence[str],
    *,
    binary: PathInput | None = None,
) -> subprocess.CompletedProcess[str]:
    exe = find_binary(binary)
    return subprocess.run(
        [str(exe), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )


def version(*, binary: PathInput | None = None) -> str:
    """Return the version reported by ``chronicle-cli --version``."""
    proc = _run_cli(["--version"], binary=binary)
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or "failed to query chronicle-cli version",
            exit_code=proc.returncode,
        )
    parts = proc.stdout.strip().split()
    return parts[-1] if parts else proc.stdout.strip()


def bench(
    *,
    ops: int = 100_000,
    seed: int = 0,
    binary: PathInput | None = None,
) -> BenchReport:
    """Run ``chronicle-cli bench --json`` and return a typed ``BenchReport``."""
    proc = _run_cli(
        ["bench", "--ops", str(ops), "--seed", str(seed), "--json"],
        binary=binary,
    )
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or "chronicle-cli bench failed",
            exit_code=proc.returncode,
        )
    data = json.loads(proc.stdout)
    return BenchReport(
        seed=int(data["seed"]),
        ops=int(data["ops"]),
        applied=int(data["applied"]),
        rejected=int(data["rejected"]),
        trades=int(data["trades"]),
        live_orders=int(data["live_orders"]),
        best_bid=data["best_bid"],
        best_ask=data["best_ask"],
    )


def _source_args(
    wal_path: PathInput,
    *,
    snapshot_path: PathInput | None,
    config: BookSpec | None,
) -> list[str]:
    if (snapshot_path is None) == (config is None):
        raise ChronicleError(
            "supply exactly one of snapshot_path or config",
            exit_code=2,
        )
    args = ["--wal", os.fspath(wal_path)]
    if snapshot_path is not None:
        args.extend(["--snapshot", os.fspath(snapshot_path)])
    elif config is not None:
        args.extend(config.to_cli_args())
    return args


def _parse_kv_tokens(tokens: Sequence[str]) -> dict[str, int | str]:
    parsed: dict[str, int | str] = {}
    for token in tokens:
        if "=" not in token:
            continue
        key, raw_val = token.split("=", 1)
        try:
            parsed[key] = int(raw_val)
        except ValueError:
            parsed[key] = raw_val
    return parsed


def verify(
    wal_path: PathInput,
    *,
    snapshot_path: PathInput | None = None,
    config: BookSpec | None = None,
    binary: PathInput | None = None,
) -> VerifyReport:
    """Verify sequence continuity and CRC32C health of a WAL and optional snapshot."""
    args = ["verify", *_source_args(wal_path, snapshot_path=snapshot_path, config=config)]
    proc = _run_cli(args, binary=binary)
    if proc.returncode == 2:
        raise ChronicleError(
            proc.stderr.strip() or "chronicle-cli verify failed",
            exit_code=2,
        )
    snap_seq: int | None = None
    snap_live: int | None = None
    frames = 0
    discarded = 0
    last_seq = 0
    detail = proc.stdout.strip()
    for line in proc.stdout.splitlines():
        if line.startswith("snapshot: ok "):
            kv = _parse_kv_tokens(line[len("snapshot: ok ") :].split())
            snap_seq = int(kv["last_included_seq"])
            snap_live = int(kv["live_orders"])
        elif line.startswith("wal: ok "):
            kv = _parse_kv_tokens(line[len("wal: ok ") :].split())
            frames = int(kv["frames"])
            discarded = int(kv["discarded"])
            last_seq = int(kv["last_seq"])
        elif line.startswith("verdict: "):
            detail = line[len("verdict: ") :].strip()
    return VerifyReport(
        ok=(proc.returncode == 0),
        frames=frames,
        discarded_trailing_bytes=discarded,
        last_seq=last_seq,
        snapshot_last_included_seq=snap_seq,
        snapshot_live_orders=snap_live,
        detail=detail,
        exit_code=proc.returncode,
    )


def replay(
    wal_path: PathInput,
    *,
    snapshot_path: PathInput | None = None,
    config: BookSpec | None = None,
    seed: int = 0,
    binary: PathInput | None = None,
) -> ReplayReport:
    """Replay a WAL through ``chronicle-cli replay`` and parse execution events."""
    args = [
        "replay",
        "--seed",
        str(seed),
        *_source_args(wal_path, snapshot_path=snapshot_path, config=config),
    ]
    proc = _run_cli(args, binary=binary)
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or proc.stdout.strip() or "chronicle-cli replay failed",
            exit_code=proc.returncode,
        )
    lines = [ln.strip() for ln in proc.stdout.splitlines() if ln.strip()]
    reported_seed = seed
    events: list[ReplayEvent] = []
    for line in lines:
        if line.startswith("seed="):
            reported_seed = int(line.split("=", 1)[1])
            continue
        parts = line.split()
        seq = int(parts[0])
        kind = parts[1]
        events.append(ReplayEvent(seq=seq, kind=kind, fields=_parse_kv_tokens(parts[2:])))
    return ReplayReport(seed=reported_seed, events=tuple(events))


def snapshot(
    wal_path: PathInput,
    out_path: PathInput,
    *,
    snapshot_path: PathInput | None = None,
    config: BookSpec | None = None,
    binary: PathInput | None = None,
) -> SnapshotReport:
    """Write an atomic snapshot from ``wal_path`` to ``out_path``."""
    args = [
        "snapshot",
        *_source_args(wal_path, snapshot_path=snapshot_path, config=config),
        "--out",
        os.fspath(out_path),
    ]
    proc = _run_cli(args, binary=binary)
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or "chronicle-cli snapshot failed",
            exit_code=proc.returncode,
        )
    kv = _parse_kv_tokens(proc.stdout.strip().split()[2:])
    return SnapshotReport(
        last_included_seq=int(kv["last_included_seq"]),
        live_orders=int(kv["live_orders"]),
        out_path=Path(out_path),
    )


def checkpoint(
    wal_path: PathInput,
    out_path: PathInput,
    *,
    snapshot_path: PathInput | None = None,
    config: BookSpec | None = None,
    binary: PathInput | None = None,
) -> CheckpointReport:
    """Write a verified snapshot and archive ``wal_path`` under its covered sequence."""
    args = [
        "checkpoint",
        *_source_args(wal_path, snapshot_path=snapshot_path, config=config),
        "--out",
        os.fspath(out_path),
    ]
    proc = _run_cli(args, binary=binary)
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or "chronicle-cli checkpoint failed",
            exit_code=proc.returncode,
        )
    kv = _parse_kv_tokens(proc.stdout.strip().split()[2:])
    return CheckpointReport(
        last_included_seq=int(kv["last_included_seq"]),
        live_orders=int(kv["live_orders"]),
        out_path=Path(out_path),
        archived_log=Path(str(kv["archived_log"])),
    )
