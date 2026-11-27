#!/usr/bin/env python3
"""End-to-end Python SDK walkthrough: CWAL encoding, snapshot inspection, and replay."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
if importlib.util.find_spec("chronicle_lob") is None:
    sys.path.insert(0, str(ROOT / "python"))

import chronicle_lob  # noqa: E402


def main() -> None:
    spec = chronicle_lob.BookSpec(
        max_orders=128,
        grid_len=640,
        tick_size=1,
        grid_start_tick=1,
        default_stp="cancel-incoming",
    )

    # 1. Build a 7-command multi-participant market session in CWAL v1 wire format.
    wal = chronicle_lob.WalBuilder(start_seq=1)
    wal.add(order_id=101, participant=10, side="bid", price=100, qty=12, order_type="limit")
    wal.add(
        order_id=102,
        participant=20,
        side="ask",
        price=103,
        qty=8,
        order_type="post-only",
        stp="cancel-both",
    )
    wal.add(order_id=103, participant=20, side="ask", price=102, qty=10, order_type="post-only")
    wal.add(order_id=104, participant=30, side="bid", price=102, qty=14, order_type="ioc")
    wal.amend(order_id=101, new_price=101, new_qty=15)
    wal.cancel(order_id=102)
    wal.add(order_id=105, participant=40, side="ask", price=101, qty=6, order_type="limit")

    # 2. Decode and verify all CWAL v1 frames purely in Python.
    raw_bytes = wal.to_bytes()
    frames = chronicle_lob.read_wal_frames(raw_bytes)
    assert len(frames) == 7, f"expected 7 frames, got {len(frames)}"
    assert [f.sequence for f in frames] == [1, 2, 3, 4, 5, 6, 7]
    assert [f.command_type for f in frames] == [
        "add",
        "add",
        "add",
        "add",
        "amend",
        "cancel",
        "add",
    ]

    with tempfile.TemporaryDirectory(prefix="chronicle-session-") as tmp:
        tmp_dir = Path(tmp)
        wal_path = wal.write(tmp_dir / "session.wal")
        snap_path = tmp_dir / "session.snap"

        # 3. Verify the WAL via chronicle-cli and write an atomic CSNP v1 snapshot.
        ver = chronicle_lob.verify(wal_path, config=spec)
        assert ver.ok and ver.frames == 7 and ver.discarded_trailing_bytes == 0
        assert ver.last_seq == 7

        snap_report = chronicle_lob.snapshot(wal_path, snap_path, config=spec)
        assert snap_report.last_included_seq == 7
        assert snap_report.live_orders == 1

        # 4. Validate the CSNP v1 binary snapshot header and trailing CRC32C in Python.
        header = chronicle_lob.read_snapshot_header(snap_path)
        expected_snap_bytes = 68 + 8 * spec.max_orders + 20 * spec.grid_len + 56 * 1
        assert header.version == 1
        assert header.last_included_seq == 7
        assert header.next_seq == 8
        assert header.live_order_count == 1
        assert header.config == spec
        assert header.byte_len == expected_snap_bytes

        # 5. Replay the session and assert the exact deterministic event sequence.
        rep = chronicle_lob.replay(wal_path, config=spec, seed=42)
        assert rep.seed == 42
        kinds = [ev.kind for ev in rep.events]
        assert kinds == [
            "accepted",
            "accepted",
            "accepted",
            "trade",
            "filled",
            "expired",
            "amended",
            "cancelled",
            "trade",
            "filled",
        ], f"unexpected event stream: {kinds}"

        trades = [ev for ev in rep.events if ev.kind == "trade"]
        assert len(trades) == 2
        assert trades[0].fields == {"maker": 103, "taker": 104, "price": 102, "qty": 10}
        assert trades[1].fields == {"maker": 101, "taker": 105, "price": 101, "qty": 6}

        # 6. Append a post-snapshot continuation frame (seq 8) and replay across the seam.
        tail = chronicle_lob.WalBuilder(start_seq=header.next_seq)
        tail.cancel_by_participant(participant=10)
        tail_path = tail.write(tmp_dir / "tail.wal")

        seam_ver = chronicle_lob.verify(tail_path, snapshot_path=snap_path)
        assert seam_ver.ok and seam_ver.last_seq == 8
        assert seam_ver.snapshot_last_included_seq == 7

        seam_rep = chronicle_lob.replay(tail_path, snapshot_path=snap_path, seed=42)
        assert len(seam_rep.events) == 1
        assert seam_rep.events[0].seq == 8
        assert seam_rep.events[0].kind == "cancelled"
        assert seam_rep.events[0].fields == {"order": 101, "reason": "by-participant"}

        print(
            f"session ok: frames={len(frames)} raw_bytes={len(raw_bytes)} "
            f"snapshot_bytes={header.byte_len} trades={len(trades)} "
            f"post_seam_seq={seam_ver.last_seq}"
        )


if __name__ == "__main__":
    main()
