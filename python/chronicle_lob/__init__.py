"""Typed Python SDK, CWAL/CSNP binary codecs, and CLI wrapper for ``chronicle-lob``."""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import sysconfig

__all__ = [
    "__version__",
    "BenchReport",
    "BookSpec",
    "CheckpointReport",
    "ChronicleError",
    "ReplayEvent",
    "ReplayReport",
    "SnapshotHeader",
    "SnapshotReport",
    "VerifyReport",
    "WalBuilder",
    "WalFrame",
    "bench",
    "checkpoint",
    "completions",
    "crc32c",
    "find_binary",
    "iter_wal_frames",
    "read_snapshot_header",
    "read_wal_frames",
    "replay",
    "snapshot",
    "verify",
    "version",
]

__version__ = "0.3.3"

PathInput = str | os.PathLike[str]

_WAL_DOMAIN = b"chronicle-wal/v1\x00"
_SNAPSHOT_DOMAIN = b"chronicle-snapshot/v1\x00"
_MAGIC = b"CWAL"
_SNAPSHOT_MAGIC = b"CSNP"
_WAL_HEADER_LEN = 32
_WAL_MAX_PAYLOAD_LEN = 4096
_SNAPSHOT_HEADER_LEN = 64
_SNAPSHOT_TRAILER_LEN = 4
_U64_MAX = 0xFFFF_FFFF_FFFF_FFFF
_U32_NIL = 0xFFFF_FFFF
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
_BYTE_TO_SIDE = {0: "bid", 1: "ask"}
_ORDER_TYPE_BYTES = {"limit": 0, "ioc": 1, "fok": 2, "post-only": 3, "post_only": 3}
_BYTE_TO_ORDER_TYPE = {0: "limit", 1: "ioc", 2: "fok", 3: "post-only"}
_STP_BYTES = {
    None: 255,
    "cancel-incoming": 0,
    "cancel_incoming": 0,
    "cancel-resting": 1,
    "cancel_resting": 1,
    "cancel-both": 2,
    "cancel_both": 2,
}
_BYTE_TO_STP = {
    0: "cancel-incoming",
    1: "cancel-resting",
    2: "cancel-both",
}
_KIND_SPECS: dict[int, tuple[str, int]] = {
    1: ("add", 40),
    2: ("cancel", 16),
    3: ("mass-cancel", 16),
    4: ("amend", 32),
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


def _snapshot_crc(preceding: bytes | memoryview) -> int:
    crc = 0xFFFFFFFF
    table = _CRC32C_TABLE
    for chunk in (_SNAPSHOT_DOMAIN, preceding):
        for byte in chunk:
            crc = table[(crc ^ byte) & 0xFF] ^ (crc >> 8)
    return (~crc) & 0xFFFFFFFF


def _read_source_bytes(source: PathInput | bytes) -> bytes:
    if isinstance(source, (bytes, bytearray, memoryview)):
        return bytes(source)
    target = Path(source)
    try:
        return target.read_bytes()
    except OSError as exc:
        raise ChronicleError(f"cannot read {target}: {exc}", exit_code=2) from exc


class ChronicleError(RuntimeError):
    """Raised when ``chronicle-cli`` or binary decoders report corruption or invalid inputs."""

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
class WalFrame:
    """One decoded and CRC-verified ``CWAL`` v1 command frame."""

    sequence: int
    command_type: str
    order_id: int | None
    participant: int | None
    side: str | None
    order_type: str | None
    stp: str | None
    price: int | None
    qty: int | None
    payload: bytes
    crc32c: int


@dataclass(frozen=True, slots=True)
class SnapshotHeader:
    """Validated ``CSNP`` v1 snapshot header and verified trailing checksum."""

    version: int
    last_included_seq: int
    next_seq: int
    live_order_count: int
    config: BookSpec
    byte_len: int
    crc32c: int


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


def _decode_wal_payload(
    kind: int,
    command_type: str,
    payload: bytes,
    seq: int,
    stored_crc: int,
    offset: int,
) -> WalFrame:
    if kind == 1:
        if payload[15] != 0:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: nonzero add.reserved byte",
                exit_code=1,
            )
        if payload[32:40] != b"\x00" * 8:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: nonzero add.reserved_tail bytes",
                exit_code=1,
            )
        order_id, participant, side_b, type_b, stp_b = struct.unpack_from(
            "<QIBBB", payload, 0
        )
        price, qty = struct.unpack_from("<QQ", payload, 16)
        side = _BYTE_TO_SIDE.get(side_b)
        if side is None:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: invalid add.side byte {side_b}",
                exit_code=1,
            )
        order_type = _BYTE_TO_ORDER_TYPE.get(type_b)
        if order_type is None:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: invalid add.order_type byte {type_b}",
                exit_code=1,
            )
        if stp_b == 255:
            stp: str | None = None
        else:
            stp = _BYTE_TO_STP.get(stp_b)
            if stp is None:
                raise ChronicleError(
                    f"corrupt WAL frame at offset {offset}: invalid add.stp byte {stp_b}",
                    exit_code=1,
                )
        return WalFrame(
            sequence=seq,
            command_type=command_type,
            order_id=order_id,
            participant=participant,
            side=side,
            order_type=order_type,
            stp=stp,
            price=price,
            qty=qty,
            payload=payload,
            crc32c=stored_crc,
        )
    if kind == 2:
        if payload[8:16] != b"\x00" * 8:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: nonzero cancel.reserved bytes",
                exit_code=1,
            )
        (order_id,) = struct.unpack_from("<Q", payload, 0)
        return WalFrame(
            sequence=seq,
            command_type=command_type,
            order_id=order_id,
            participant=None,
            side=None,
            order_type=None,
            stp=None,
            price=None,
            qty=None,
            payload=payload,
            crc32c=stored_crc,
        )
    if kind == 3:
        if payload[4:16] != b"\x00" * 12:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: nonzero cancel_by_participant.reserved bytes",
                exit_code=1,
            )
        (participant,) = struct.unpack_from("<I", payload, 0)
        return WalFrame(
            sequence=seq,
            command_type=command_type,
            order_id=None,
            participant=participant,
            side=None,
            order_type=None,
            stp=None,
            price=None,
            qty=None,
            payload=payload,
            crc32c=stored_crc,
        )
    if payload[24:32] != b"\x00" * 8:
        raise ChronicleError(
            f"corrupt WAL frame at offset {offset}: nonzero amend.reserved bytes",
            exit_code=1,
        )
    order_id, new_price, new_qty = struct.unpack_from("<QQQ", payload, 0)
    return WalFrame(
        sequence=seq,
        command_type=command_type,
        order_id=order_id,
        participant=None,
        side=None,
        order_type=None,
        stp=None,
        price=new_price,
        qty=new_qty,
        payload=payload,
        crc32c=stored_crc,
    )


def _iter_wal_frames_from_bytes(data: bytes) -> Iterator[WalFrame]:
    total_len = len(data)
    offset = 0
    expected_seq: int | None = None

    while offset < total_len:
        rem = total_len - offset
        if rem < 4:
            prefix = data[offset : offset + rem]
            if prefix != _MAGIC[:rem]:
                raise ChronicleError(
                    f"corrupt WAL frame at offset {offset}: bad magic {prefix!r}",
                    exit_code=1,
                )
            raise ChronicleError(
                f"truncated WAL header at offset {offset}: expected 32 bytes, found {rem}",
                exit_code=1,
            )
        magic = data[offset : offset + 4]
        if magic != _MAGIC:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: bad magic {magic!r}",
                exit_code=1,
            )
        if rem >= 6:
            (version_preview,) = struct.unpack_from("<H", data, offset + 4)
            if version_preview != 1:
                raise ChronicleError(
                    f"corrupt WAL frame at offset {offset}: unsupported version {version_preview}",
                    exit_code=1,
                )
        elif rem == 5 and data[offset + 4] != 1:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: unsupported version {data[offset + 4]}",
                exit_code=1,
            )
        if rem < _WAL_HEADER_LEN:
            raise ChronicleError(
                f"truncated WAL header at offset {offset}: expected 32 bytes, found {rem}",
                exit_code=1,
            )

        (
            _magic,
            version,
            header_len,
            seq,
            payload_len,
            kind,
            reserved_bytes,
            stored_crc,
        ) = struct.unpack_from("<4sHHQIB7sI", data, offset)

        if version != 1:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: unsupported version {version}",
                exit_code=1,
            )
        if header_len != _WAL_HEADER_LEN:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: bad header_len {header_len}",
                exit_code=1,
            )
        if seq == 0:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: zero sequence number",
                exit_code=1,
            )
        if payload_len > _WAL_MAX_PAYLOAD_LEN:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: payload_len {payload_len} exceeds {_WAL_MAX_PAYLOAD_LEN}",
                exit_code=1,
            )
        spec = _KIND_SPECS.get(kind)
        if spec is None:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: unknown command tag {kind}",
                exit_code=1,
            )
        command_type, expected_payload_len = spec
        if payload_len != expected_payload_len:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: payload_len {payload_len} != {expected_payload_len} for {command_type}",
                exit_code=1,
            )
        if reserved_bytes != b"\x00" * 7:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: nonzero header reserved bytes",
                exit_code=1,
            )

        frame_len = _WAL_HEADER_LEN + payload_len
        if rem < frame_len:
            raise ChronicleError(
                f"truncated WAL payload at offset {offset}: expected {frame_len} bytes, found {rem}",
                exit_code=1,
            )

        header_prefix = data[offset : offset + 28]
        payload = data[offset + _WAL_HEADER_LEN : offset + frame_len]
        computed_crc = _frame_crc(header_prefix, payload)
        if computed_crc != stored_crc:
            raise ChronicleError(
                f"corrupt WAL frame at offset {offset}: CRC32C mismatch (expected 0x{computed_crc:08x}, found 0x{stored_crc:08x})",
                exit_code=1,
            )

        frame = _decode_wal_payload(
            kind, command_type, payload, seq, stored_crc, offset
        )
        if expected_seq is not None and seq != expected_seq:
            raise ChronicleError(
                f"WAL sequence gap at offset {offset}: expected {expected_seq}, found {seq}",
                exit_code=1,
            )
        expected_seq = seq + 1
        yield frame
        offset += frame_len


def iter_wal_frames(source: PathInput | bytes) -> Iterator[WalFrame]:
    """Decode and CRC-verify ``CWAL`` v1 frames from ``source`` lazily."""
    data = _read_source_bytes(source)
    return _iter_wal_frames_from_bytes(data)


def read_wal_frames(source: PathInput | bytes) -> tuple[WalFrame, ...]:
    """Decode and CRC-verify all ``CWAL`` v1 frames from ``source``."""
    return tuple(iter_wal_frames(source))


def read_snapshot_header(source: PathInput | bytes) -> SnapshotHeader:
    """Validate a ``CSNP`` v1 snapshot header and verify its trailing CRC32C."""
    data = _read_source_bytes(source)
    actual_len = len(data)
    min_len = _SNAPSHOT_HEADER_LEN + _SNAPSHOT_TRAILER_LEN
    if actual_len == 0:
        raise ChronicleError(
            f"truncated snapshot: expected at least {min_len} bytes, found 0",
            exit_code=1,
        )
    prefix = data[: min(actual_len, 4)]
    if prefix != _SNAPSHOT_MAGIC[: len(prefix)]:
        raise ChronicleError(
            f"corrupt snapshot: bad magic {prefix!r}",
            exit_code=1,
        )
    if actual_len < min_len:
        raise ChronicleError(
            f"truncated snapshot: expected at least {min_len} bytes, found {actual_len}",
            exit_code=1,
        )

    (
        magic,
        version,
        header_len,
        last_included_seq,
        next_seq,
        live_order_count,
        max_orders,
        tick_size,
        grid_start_tick,
        grid_len,
        default_stp_raw,
        free_list_head,
        reserved,
    ) = struct.unpack_from("<4sHHQQIIQQIIII", data, 0)

    if magic != _SNAPSHOT_MAGIC:
        raise ChronicleError(f"corrupt snapshot: bad magic {magic!r}", exit_code=1)
    if version != 1:
        raise ChronicleError(
            f"corrupt snapshot: unsupported version {version}", exit_code=1
        )
    if header_len != _SNAPSHOT_HEADER_LEN:
        raise ChronicleError(
            f"corrupt snapshot: bad header_len {header_len}", exit_code=1
        )

    expected_len = (
        68 + 8 * max_orders + 20 * grid_len + 56 * live_order_count
    )
    if actual_len != expected_len:
        raise ChronicleError(
            f"corrupt snapshot: byte length mismatch (expected {expected_len}, found {actual_len})",
            exit_code=1,
        )
    if last_included_seq + 1 != next_seq:
        raise ChronicleError(
            f"corrupt snapshot: next_seq {next_seq} != last_included_seq + 1 ({last_included_seq + 1})",
            exit_code=1,
        )
    if reserved != 0:
        raise ChronicleError(
            "corrupt snapshot: nonzero header.reserved field", exit_code=1
        )
    default_stp = _BYTE_TO_STP.get(default_stp_raw)
    if default_stp is None:
        raise ChronicleError(
            f"corrupt snapshot: invalid default_stp {default_stp_raw}", exit_code=1
        )
    if max_orders < 1 or max_orders > (1 << 31) - 2:
        raise ChronicleError(
            f"corrupt snapshot: invalid max_orders {max_orders}", exit_code=1
        )
    if grid_len < 64 or grid_len % 64 != 0 or grid_len > (1 << 22):
        raise ChronicleError(
            f"corrupt snapshot: invalid grid_len {grid_len}", exit_code=1
        )
    if tick_size < 1:
        raise ChronicleError(
            f"corrupt snapshot: invalid tick_size {tick_size}", exit_code=1
        )
    grid_end_tick = grid_start_tick + grid_len
    if grid_end_tick > _U64_MAX or (grid_end_tick - 1) * tick_size > _U64_MAX:
        raise ChronicleError(
            "corrupt snapshot: grid price range overflows u64", exit_code=1
        )
    if live_order_count > max_orders:
        raise ChronicleError(
            f"corrupt snapshot: live_order_count {live_order_count} exceeds max_orders {max_orders}",
            exit_code=1,
        )
    if free_list_head != _U32_NIL and free_list_head >= max_orders:
        raise ChronicleError(
            f"corrupt snapshot: invalid free_list_head {free_list_head}", exit_code=1
        )

    (stored_crc,) = struct.unpack_from("<I", data, actual_len - _SNAPSHOT_TRAILER_LEN)
    computed_crc = _snapshot_crc(memoryview(data)[: actual_len - _SNAPSHOT_TRAILER_LEN])
    if computed_crc != stored_crc:
        raise ChronicleError(
            f"corrupt snapshot: CRC32C mismatch (expected 0x{computed_crc:08x}, found 0x{stored_crc:08x})",
            exit_code=1,
        )

    return SnapshotHeader(
        version=version,
        last_included_seq=last_included_seq,
        next_seq=next_seq,
        live_order_count=live_order_count,
        config=BookSpec(
            max_orders=max_orders,
            grid_len=grid_len,
            tick_size=tick_size,
            grid_start_tick=grid_start_tick,
            default_stp=default_stp,
        ),
        byte_len=actual_len,
        crc32c=stored_crc,
    )


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


def completions(
    shell: str = "bash",
    *,
    binary: PathInput | None = None,
) -> str:
    """Return the shell completion script generated by ``chronicle-cli completions``."""
    proc = _run_cli(["completions", shell], binary=binary)
    if proc.returncode != 0:
        raise ChronicleError(
            proc.stderr.strip() or f"chronicle-cli completions failed for {shell!r}",
            exit_code=proc.returncode,
        )
    return proc.stdout


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
