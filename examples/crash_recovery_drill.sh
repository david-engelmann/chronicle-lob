#!/bin/sh
# End-to-end crash-recovery and checkpoint drill using chronicle-cli in a
# temporary directory:
#   1. Generate a valid 4-frame CWAL v1 log and a 2-frame continuation tail.
#   2. Simulate an interrupted crash tail (19 bytes of frame 5 header prefix).
#   3. Verify that chronicle-cli reports a clean crash boundary (exit 0, discarded=19)
#      while rejecting mid-log byte tampering (exit 1).
#   4. Take a snapshot, run a verified checkpoint (archiving the covered log),
#      replay continuation frames across the checkpoint seam, and run bench --json.
set -eu

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd)"

find_cli() {
  if [ -n "${CHRONICLE_CLI_BIN:-}" ] && [ -f "${CHRONICLE_CLI_BIN}" ]; then
    printf '%s\n' "${CHRONICLE_CLI_BIN}"
    return 0
  fi
  for candidate in \
    "${REPO_ROOT}/target/release/chronicle-cli" \
    "${REPO_ROOT}/target/release/chronicle-cli.exe" \
    "${REPO_ROOT}/target/debug/chronicle-cli" \
    "${REPO_ROOT}/target/debug/chronicle-cli.exe"
  do
    if [ -f "${candidate}" ]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done
  if command -v chronicle-cli >/dev/null 2>&1; then
    command -v chronicle-cli
    return 0
  fi
  printf 'crash_recovery_drill.sh: could not find chronicle-cli; build it first with:\n' >&2
  printf '  cargo build --locked -p chronicle-lob-cli --bin chronicle-cli\n' >&2
  exit 2
}

CLI_BIN="$(find_cli)"
WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT INT TERM

# Synthesize deterministic CWAL v1 fixtures:
# - crashed.wal: 4 complete frames (seq 1..4) + 19-byte torn CWAL v1 header prefix
# - corrupt.wal: 4 complete frames with byte 12 flipped (mid-frame corruption)
# - next.wal:    2 continuation frames (seq 5..6) starting right after seq 4
python3 - "${REPO_ROOT}/python" "${WORK_DIR}" <<'PY'
from pathlib import Path
import struct
import sys

sys.path.insert(0, sys.argv[1])
import chronicle_lob  # noqa: E402

work = Path(sys.argv[2])

base = chronicle_lob.WalBuilder(start_seq=1)
base.add(order_id=1, participant=10, side="bid", price=100, qty=10)
base.add(order_id=2, participant=20, side="ask", price=102, qty=8)
base.add(order_id=3, participant=30, side="bid", price=102, qty=5)
base.amend(order_id=1, new_price=101, new_qty=9)
clean_bytes = base.to_bytes()

# 19-byte prefix of a valid CWAL v1 frame 5 header interrupted mid-write.
full_header_prefix = struct.pack("<4sHHQIB7s", b"CWAL", 1, 32, 5, 16, 2, b"\x00" * 7)
torn_prefix = full_header_prefix[:19]
(work / "crashed.wal").write_bytes(clean_bytes + torn_prefix)

corrupt = bytearray(clean_bytes)
corrupt[12] ^= 0x01
(work / "corrupt.wal").write_bytes(bytes(corrupt))

tail = chronicle_lob.WalBuilder(start_seq=5)
tail.add(order_id=4, participant=40, side="ask", price=101, qty=4)
tail.cancel(order_id=2)
tail.write(work / "next.wal")
PY

# 1. Verify crashed.wal: torn 19-byte tail must be reported cleanly (exit 0).
VERIFY_OUT="$("${CLI_BIN}" verify \
  --wal "${WORK_DIR}/crashed.wal" \
  --max-orders 64 --grid-len 640 --tick-size 1 --grid-start-tick 1)"
case "${VERIFY_OUT}" in
  *"frames=4 discarded=19 last_seq=4"*"verdict: ok"*) ;;
  *)
    printf 'unexpected verify output on crashed.wal:\n%s\n' "${VERIFY_OUT}" >&2
    exit 1
    ;;
esac

# 2. Verify corrupt.wal: mid-frame corruption must fail with exit code 1.
set +e
CORRUPT_OUT="$("${CLI_BIN}" verify \
  --wal "${WORK_DIR}/corrupt.wal" \
  --max-orders 64 --grid-len 640 --tick-size 1 --grid-start-tick 1 2>&1)"
CORRUPT_CODE=$?
set -e
if [ "${CORRUPT_CODE}" -ne 1 ]; then
  printf 'expected exit code 1 on corrupt.wal, got %s:\n%s\n' "${CORRUPT_CODE}" "${CORRUPT_OUT}" >&2
  exit 1
fi

# 3. Take an atomic base snapshot from crashed.wal, then checkpoint it.
"${CLI_BIN}" snapshot \
  --wal "${WORK_DIR}/crashed.wal" \
  --out "${WORK_DIR}/base.snapshot" \
  --max-orders 64 --grid-len 640 --tick-size 1 --grid-start-tick 1 >/dev/null

CHECKPOINT_OUT="$("${CLI_BIN}" checkpoint \
  --wal "${WORK_DIR}/crashed.wal" \
  --snapshot "${WORK_DIR}/base.snapshot" \
  --out "${WORK_DIR}/book.snapshot")"
if [ ! -f "${WORK_DIR}/crashed.wal.archive-4" ]; then
  printf 'expected archived log crashed.wal.archive-4 after checkpoint:\n%s\n' "${CHECKPOINT_OUT}" >&2
  exit 1
fi

# 4. Replay the post-checkpoint continuation WAL (seq 5..6) from book.snapshot.
REPLAY_OUT="$("${CLI_BIN}" replay \
  --seed 42 \
  --wal "${WORK_DIR}/next.wal" \
  --snapshot "${WORK_DIR}/book.snapshot")"
case "${REPLAY_OUT}" in
  *"seed=42"*"5 trade maker=1 taker=4 price=101 qty=4"*"6 cancelled order=2 reason=requested"*) ;;
  *)
    printf 'unexpected replay output across checkpoint seam:\n%s\n' "${REPLAY_OUT}" >&2
    exit 1
    ;;
esac

# 5. Smoke-check deterministic JSON output from bench.
BENCH_JSON="$("${CLI_BIN}" bench --ops 1000 --seed 42 --json)"
case "${BENCH_JSON}" in
  *'"ops":1000'*'"seed":42'*) ;;
  *)
    printf 'unexpected bench --json output:\n%s\n' "${BENCH_JSON}" >&2
    exit 1
    ;;
esac

printf 'crash recovery drill ok: discarded=19 corrupt_exit=%d checkpoint_seq=4 bench_ops=1000\n' "${CORRUPT_CODE}"
