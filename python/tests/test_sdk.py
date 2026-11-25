from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
if importlib.util.find_spec("chronicle_lob") is None:
    sys.path.insert(0, str(ROOT / "python"))

import chronicle_lob  # noqa: E402


class ChronicleLobSdkTests(unittest.TestCase):
    def test_version_matches_package(self) -> None:
        self.assertEqual(chronicle_lob.version(), chronicle_lob.__version__)

    def test_crc32c_golden_vector(self) -> None:
        self.assertEqual(chronicle_lob.crc32c(b"123456789"), 0xE3069283)

    def test_bench_determinism(self) -> None:
        first = chronicle_lob.bench(ops=5_000, seed=42)
        second = chronicle_lob.bench(ops=5_000, seed=42)
        self.assertEqual(first, second)
        self.assertEqual(first.ops, 5_000)
        self.assertEqual(first.applied + first.rejected, 5_000)

    def test_wal_builder_verify_replay_and_checkpoint(self) -> None:
        config = chronicle_lob.BookSpec(
            max_orders=64,
            grid_len=640,
            tick_size=1,
            grid_start_tick=1,
        )
        builder = chronicle_lob.WalBuilder()
        builder.add(order_id=1, participant=10, side="bid", price=100, qty=5)
        builder.add(order_id=2, participant=20, side="ask", price=102, qty=4)
        builder.add(order_id=3, participant=30, side="bid", price=102, qty=3)

        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            wal_path = builder.write(tmp_dir / "session.wal")

            ver = chronicle_lob.verify(wal_path, config=config)
            self.assertTrue(ver.ok)
            self.assertEqual(ver.frames, 3)
            self.assertEqual(ver.last_seq, 3)

            rep = chronicle_lob.replay(wal_path, config=config, seed=7)
            self.assertEqual(rep.seed, 7)
            kinds = [ev.kind for ev in rep.events]
            self.assertEqual(kinds, ["accepted", "accepted", "trade", "filled"])
            self.assertEqual(rep.events[2].fields["price"], 102)
            self.assertEqual(rep.events[2].fields["qty"], 3)

            snap_path = tmp_dir / "book.snapshot"
            cp = chronicle_lob.checkpoint(wal_path, snap_path, config=config)
            self.assertEqual(cp.last_included_seq, 3)
            self.assertEqual(cp.live_orders, 2)
            self.assertTrue(cp.out_path.is_file())
            self.assertTrue(cp.archived_log.is_file())


if __name__ == "__main__":
    unittest.main()
